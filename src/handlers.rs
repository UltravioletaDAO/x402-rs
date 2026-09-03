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
use axum::extract::{Extension, Path, Query, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{response::IntoResponse, Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, error, info, instrument, warn};

use std::collections::HashMap;
use std::sync::Arc;

use crate::chain::evm::MetaEvmProvider;
use crate::chain::{FacilitatorLocalError, NetworkProvider, NetworkProviderOps};
use crate::discovery::{DiscoveryError, DiscoveryRegistry};
use crate::erc8004::register_jobs;
use crate::erc8004::solana as solana_erc8004;
use crate::erc8004::{
    get_contracts, is_erc8004_supported, parse_agent_id_value, supported_network_names,
    AgentIdentity, AppendResponseRequest, AtomStatsResponse, FeedbackEntry, FeedbackRequest,
    FeedbackResponse, IIdentityRegistry, IReputationRegistry, MetadataEntry, MetadataEntryParam,
    RegisterAgentRequest, RegisterAgentResponse, ReputationResponse, ReputationSummary,
    RevokeFeedbackRequest,
};
use crate::facilitator::Facilitator;
use crate::fhe_proxy::FheProxy;
use crate::idempotency_store::{hash_request_body, IdempotencyRecord, IDEMPOTENCY_TTL_SECONDS};
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{
    ErrorResponse, FacilitatorErrorReason, MixedAddress, SettleRequest, SettleResponse,
    VerifyRequest, VerifyResponse,
};
use crate::types_v2::{
    DiscoveryFilters, DiscoveryResource, RegisterResourceRequest, SettleRequestEnvelope,
    SupportedPaymentKindsResponseV1ToV2, VerifyRequestEnvelope,
};
use alloy::providers::Provider as _;
use solana_sdk::signer::Signer as _;

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

/// Verify + settle routes that should be rate-limited.
///
/// Split out of [`routes`] so `main.rs` can wrap these (and only these) in a
/// stricter `GovernorLayer`. Each `/verify` and `/settle` call burns RPC quota
/// against the configured chain providers, so an unbounded caller could drain
/// a paid plan within minutes.
pub fn verify_settle_routes<A>() -> Router<A>
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // `POST /settle` is gated; `GET /settle` returns the schema and is a read,
    // so it keeps serving from every task. They are separate routers because a
    // `.layer()` applies to every route on the router it is called on.
    let settle = Router::new()
        .route("/settle", post(post_settle::<A>))
        .layer(axum::middleware::from_fn(settle_writer_gate));

    Router::new()
        .route("/verify", get(get_verify_info))
        .route("/verify", post(post_verify::<A>))
        .route("/settle", get(get_settle_info))
        .merge(settle)
}

/// Env overrides for the identity-read rate limit. Declared here, next to the
/// routes they protect, so the numbers live in exactly one place and `main.rs`
/// reads them rather than restating them.
const ENV_IDENTITY_READ_PER_MS: &str = "IDENTITY_READ_RATE_PER_MS";
const ENV_IDENTITY_READ_BURST: &str = "IDENTITY_READ_RATE_BURST";

/// One token every 500ms = ~120 req/min sustained, burst 60.
///
/// Deliberately GENEROUS. `SmartIpKeyExtractor` buckets by client IP, and a
/// single integrator sending every one of its requests from one host lands
/// entirely in one bucket -- a tight limit throttles that whole integrator at
/// once. The observed sweep that motivated this (2026-08-29) ran ~21 req/min
/// aggregated across nine networks, so 120/min does not touch legitimate
/// traffic and only cuts off a runaway loop. A limit sized against imagined
/// abuse instead of measured traffic is how every 429 in the last bazaar
/// incident turned out to be a legitimate paginating client (2026-07-24).
const DEFAULT_IDENTITY_READ_PER_MS: u64 = 500;
const DEFAULT_IDENTITY_READ_BURST: u32 = 60;

/// Rate limit for [`identity_read_routes`], as `(per_millisecond, burst_size)`.
///
/// Note `tower_governor`'s GCRA replenishes ONE token every `per_millisecond`
/// milliseconds -- it is a period, not a rate.
pub fn identity_read_rate_limit() -> (u64, u32) {
    let per_ms = std::env::var(ENV_IDENTITY_READ_PER_MS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_IDENTITY_READ_PER_MS);
    let burst = std::env::var(ENV_IDENTITY_READ_BURST)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_IDENTITY_READ_BURST);
    (per_ms, burst)
}

/// ERC-8004 identity READ routes.
///
/// Split out of [`routes`] so `main.rs` can wrap these (and only these) in a
/// `GovernorLayer`. `/identity/{network}/owner/{address}` cannot be answered
/// from an index -- the registries expose no owner -> agentId mapping, are not
/// `ERC721Enumerable`, and SKALE caps `eth_getLogs` at 2000 blocks -- so every
/// cold lookup costs a `balanceOf`, a `totalSupply` and a Multicall3 scan.
/// Cheap per call, but unbounded it amplifies one client into a multiple of
/// that against the shared RPC budget, which is what starved `/settle` in
/// INC-2026-07-06.
pub fn identity_read_routes<A>() -> Router<A>
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    Router::new()
        .route("/identity/{network}/{agent_id}", get(get_identity::<A>))
        .route(
            "/identity/{network}/{agent_id}/metadata/{key}",
            get(get_identity_metadata::<A>),
        )
        .route(
            "/identity/{network}/total-supply",
            get(get_identity_total_supply::<A>),
        )
        .route(
            "/identity/{network}/owner/{address}",
            get(get_identity_by_owner::<A>),
        )
}

/// Env overrides for the secondary-reads rate limit. Same reasoning as
/// [`identity_read_rate_limit`]: declared once, here, so `main.rs` reads the
/// numbers instead of restating them.
const ENV_SECONDARY_READS_PER_MS: &str = "SECONDARY_READS_RATE_PER_MS";
const ENV_SECONDARY_READS_BURST: &str = "SECONDARY_READS_RATE_BURST";

/// One token every 300ms = ~200 req/min sustained, burst 100.
///
/// `/reputation/{network}/{agent_id}` and `POST /escrow/state` each cost at
/// least one RPC/contract read against the shared provider budget; `/blacklist`
/// is a local read today, but the whole point of this governor is to stop
/// relying on "cheap today" -- that was exactly the assumption that failed for
/// `/identity/owner` (2026-08-29, see [`identity_read_rate_limit`]). None of
/// these three showed up in the facilitator's latency around that incident, so
/// this is preventive, not a response to observed abuse -- which argues FOR
/// staying generous, not tight: there is no measured attack shape to size
/// against yet, only the same "one IP, many networks, in parallel" pattern
/// that hit `/identity/owner`.
///
/// Deliberately its OWN config, not a share of `discovery_read_config`: that
/// bucket (see its comment in `main.rs`) is sized against bazaar pagination --
/// a 21k-item catalog at the 100/page cap is ~212 requests back to back.
/// Folding these three routes into it would let a paginating bazaar client and
/// a reputation caller from the same IP draw down the same budget, which is
/// not a tradeoff either surface asked for.
const DEFAULT_SECONDARY_READS_PER_MS: u64 = 300;
const DEFAULT_SECONDARY_READS_BURST: u32 = 100;

/// Rate limit for [`secondary_read_routes`], as `(per_millisecond, burst_size)`.
///
/// Same GCRA semantics as [`identity_read_rate_limit`]: `per_millisecond` is a
/// replenish PERIOD, not a rate.
pub fn secondary_read_rate_limit() -> (u64, u32) {
    let per_ms = std::env::var(ENV_SECONDARY_READS_PER_MS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SECONDARY_READS_PER_MS);
    let burst = std::env::var(ENV_SECONDARY_READS_BURST)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SECONDARY_READS_BURST);
    (per_ms, burst)
}

/// Reputation, blacklist and escrow-state routes that used to live in
/// [`routes`] with no governor at all.
///
/// Split out for the same reason as [`identity_read_routes`]: so `main.rs` can
/// wrap exactly these in their own `GovernorLayer`, sized by
/// [`secondary_read_rate_limit`]. `POST /escrow/state` is a query, not a
/// write -- it never spends gas -- but it reads on-chain escrow state, so it
/// belongs here rather than with the free static routes in [`routes`].
pub fn secondary_read_routes<A>() -> Router<A>
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    Router::new()
        .route("/reputation/{network}/{agent_id}", get(get_reputation::<A>))
        .route("/blacklist", get(get_blacklist::<A>))
        .route("/escrow/state", post(post_escrow_state::<A>))
}

/// The agentic-discovery surfaces: the files an agent or a scanner fetches
/// BEFORE it knows how to call anything.
///
/// Stateless on purpose, so it merges after `.with_state(...)` in `main.rs` and
/// so its tests can build the router without a facilitator.
///
/// WHY EACH ROUTE IS SPELLED OUT
///     This crate has no `ServeDir` and never reads `static/` at runtime -- every
///     file is compiled in with `include_str!` (same pattern as `/logo.png`,
///     [`get_logo`]). A file dropped into `static/` and not listed here does not
///     exist as far as the service is concerned.
///
/// WHY THE CONTENT TYPE IS SPELLED OUT TOO
///     The checker that grades these surfaces (`c0der/scripts/agentic_check.py`)
///     requires code + content-type + a body that differs from `/`. A correct
///     document served as `text/html` scores zero, and nothing in the response
///     looks wrong.
pub fn agentic_routes() -> Router {
    Router::new()
        .route("/llms.txt", get(get_llms_txt))
        .route("/llms-full.txt", get(get_llms_full_txt))
        .route("/robots.txt", get(get_robots_txt))
        .route("/sitemap.xml", get(get_sitemap_xml))
        .route("/index.md", get(get_index_md))
        .route("/skill.md", get(get_skill_md))
        .route("/auth.md", get(get_auth_md))
        .route("/workflows.json", get(get_workflows_json))
        .route("/.well-known/agent-card.json", get(get_agent_card))
        .route("/.well-known/agent.json", get(get_agent_json_legacy))
        .route("/.well-known/x402", get(get_x402_discovery))
        .route("/.well-known/api-catalog", get(get_api_catalog))
        .route(
            "/.well-known/oauth-protected-resource",
            get(get_oauth_protected_resource),
        )
        .route(
            "/.well-known/agent-skills/index.json",
            get(get_agent_skills_index),
        )
        .route(
            "/.well-known/mcp/server-card.json",
            get(get_mcp_server_card),
        )
        .route("/.well-known/ard.json", get(get_ard))
}

/// The 405 every route answers when the path exists but the method does not.
///
/// axum's default is `405` with a zero-byte body and no content type -- the
/// same shape the 404 had, and the reason `json-error-responses` scored as
/// failed on 2026-09-02 despite `/verify` and `/settle` already rejecting bad
/// bodies in JSON.
///
/// The `Allow` header is NOT set here on purpose: axum still computes and
/// attaches it from the methods actually registered for the path (its own
/// `allow_header_with_fallback` test pins that), and a hand-written one here
/// would be a second source of truth that drifts the first time a route gains
/// a method.
#[instrument(skip_all)]
pub async fn method_not_allowed(
    method: axum::http::Method,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    let path = uri.path().to_string();
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::CONTENT_TYPE, APPLICATION_JSON_UTF8)],
        Json(json!({
            "error": format!("{method} is not supported on {path}"),
            "code": "method_not_allowed",
            "hint": "The `Allow` response header lists the methods this path \
                     accepts. Every endpoint and its methods are described at \
                     https://facilitator.ultravioletadao.xyz/openapi.json",
        })),
    )
}

/// A rate limiter's refusal, as JSON instead of a bare string.
///
/// `tower_governor` builds both of its refusals with `Response::new(String)`
/// and no content type (`tower_governor-0.8.0/src/errors.rs`), so until now the
/// single most likely error an agent meets on this service -- a 429 -- came
/// back untyped. This runs INSIDE the limiter rather than as a layer around it,
/// which is what makes it reach them: a middleware wrapping the router is
/// mounted under the `GovernorLayer`, so the limiter's own responses never pass
/// through it.
///
/// The status and every header the limiter set are preserved untouched:
/// `retry-after` and `x-ratelimit-*` are the part an agent throttles on.
pub fn rate_limit_error(error: tower_governor::GovernorError) -> Response<axum::body::Body> {
    let (mut parts, message) = error.into_response().into_parts();
    let (code, hint) = if parts.status == StatusCode::TOO_MANY_REQUESTS {
        (
            "rate_limited",
            "Wait for the number of seconds in the `retry-after` header, then \
             retry. `x-ratelimit-limit` and `x-ratelimit-remaining` report the \
             budget on every successful response, so a client can pace itself \
             instead of discovering the limit by hitting it. Limits are per \
             client IP and documented at \
             https://facilitator.ultravioletadao.xyz/skill.md",
        )
    } else {
        (
            "rate_limit_key_unavailable",
            "The rate limiter could not identify the caller: no \
             X-Forwarded-For, X-Real-IP or Forwarded header reached it. Behind \
             the production load balancer one is always present; a direct \
             connection to the service has to set it.",
        )
    };
    let body = json!({
        "error": message.trim(),
        "code": code,
        "hint": hint,
    })
    .to_string();
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(APPLICATION_JSON_UTF8),
    );
    // `Response::new(String)` carries no content-length, and the JSON body is a
    // different length than the string it replaces -- so nothing stale to clear.
    Response::from_parts(parts, axum::body::Body::from(body))
}

/// The body a nonexistent path answers with.
///
/// Markdown, opening on a heading, because that is what the readers of a 404
/// here actually are: an agent that guessed a url and now has to find the real
/// one. The four links are the documents that answer "what does this service
/// serve" -- an index, a page list, a typed API description, and the catalog
/// that ties them together.
const NOT_FOUND_MARKDOWN: &str = "\
# 404 Not Found

No route on the x402 payment facilitator serves this path.

## Where the real routes are described

- <https://facilitator.ultravioletadao.xyz/llms.txt> - what this service is, \
and every document it publishes
- <https://facilitator.ultravioletadao.xyz/sitemap.xml> - the pages a reader \
can land on
- <https://facilitator.ultravioletadao.xyz/openapi.json> - every endpoint, typed
- <https://facilitator.ultravioletadao.xyz/.well-known/api-catalog> - the RFC \
9727 catalog of the above
- <https://facilitator.ultravioletadao.xyz/skill.md> - how to call /verify and \
/settle

The two calls that are the whole contract are `POST /verify` and \
`POST /settle`. They are also MCP tools at `POST /mcp`.
";

/// The same 404, for a caller that asked for JSON.
static NOT_FOUND_JSON: Lazy<String> = Lazy::new(|| {
    json!({
        "error": "No route serves this path",
        "code": "not_found",
        "hint": "Read https://facilitator.ultravioletadao.xyz/llms.txt for what \
                 this service publishes, or https://facilitator.ultravioletadao.xyz/openapi.json \
                 for every endpoint. The two calls that matter are POST /verify \
                 and POST /settle.",
        "documentation": {
            "llms": "https://facilitator.ultravioletadao.xyz/llms.txt",
            "sitemap": "https://facilitator.ultravioletadao.xyz/sitemap.xml",
            "openapi": "https://facilitator.ultravioletadao.xyz/openapi.json",
            "apiCatalog": "https://facilitator.ultravioletadao.xyz/.well-known/api-catalog",
            "skill": "https://facilitator.ultravioletadao.xyz/skill.md",
        },
    })
    .to_string()
});

/// The fallback for every path no route claims.
///
/// WHY THIS EXISTS AT ALL
///     The status was already a real 404 -- axum's default -- but the body was
///     zero bytes with no content type, which is what both 2026-09-02 scans
///     scored as only partial credit. An agent that guesses a url and gets
///     nothing back has learned nothing; the same 404 carrying five links is a
///     recovery path.
///
/// WHY MARKDOWN IS THE DEFAULT AND NOT JSON
///     A 404 is read by whoever guessed wrong, which is far more often a crawler
///     or a person than an API client mid-call. A real API client asks for JSON,
///     and gets it.
///
/// WHERE IT SITS IN THE STACK
///     `main.rs` mounts it INSIDE a `GovernorLayer`, deliberately: an unmetered
///     404 is a free amplification surface, and a path-scanning loop is exactly
///     the traffic that finds it.
#[instrument(skip_all)]
pub async fn agent_not_found(headers: HeaderMap) -> impl IntoResponse {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok());
    // Markdown first: it is the default for a caller with no preference.
    let (media, body) =
        match crate::negotiate::choose(accept, &["text/markdown", "application/json"]) {
            crate::negotiate::Choice::Serve("application/json") => {
                ("application/json", NOT_FOUND_JSON.as_str())
            }
            // A 404 never answers 406. The caller already asked for something that
            // does not exist; refusing to say so in a format they dislike replaces
            // one dead end with a worse one.
            _ => ("text/markdown", NOT_FOUND_MARKDOWN),
        };
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", format!("{media}; charset=utf-8"))
        .header(header::VARY, "Accept, Accept-Encoding")
        .body(body.to_string())
        .unwrap()
}

/// The documents a negotiated surface can answer with, most preferred first.
///
/// `(media type, body)`. The first entry is the default: it is what a caller
/// with no `Accept`, or with `*/*`, gets, and it breaks ties between equally
/// acceptable types. See [`crate::negotiate`] for the ranking rules.
type Representations = &'static [(&'static str, &'static str)];

/// Serve one document out of several representations of the same resource,
/// chosen from the request's `Accept`.
///
/// WHY `Vary: Accept` IS NOT OPTIONAL
///     Without it a cache in front of this service keys `/` on the URL alone,
///     so whichever of the two representations lands in the cache first is
///     handed to everyone after -- Markdown rendered as raw text in a browser,
///     or a wall of HTML to the agent that asked for Markdown. It is listed
///     alongside `Accept-Encoding` because a cache that varies on one and not
///     the other has the same bug one axis over.
///
/// WHY THE 406 IS NARROW
///     Only when every representation on offer was refused. A missing `Accept`
///     or `*/*` means *no constraint*, not *nothing works*: answering 406 there
///     is the documented common mistake
///     (<https://acceptmarkdown.com/guides/returning-406>), and it would break
///     every browser and every `curl` that does not set the header.
fn negotiated_surface(headers: &HeaderMap, offers: Representations) -> Response<String> {
    negotiated_response(StatusCode::OK, headers, offers)
}

/// [`negotiated_surface`] for a response that is not a `200` -- the 404 body,
/// which is the same negotiation over a different status.
fn negotiated_response(
    status: StatusCode,
    headers: &HeaderMap,
    offers: Representations,
) -> Response<String> {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok());
    let types: Vec<&str> = offers.iter().map(|(media, _)| *media).collect();
    match crate::negotiate::choose(accept, &types) {
        crate::negotiate::Choice::Serve(media) => {
            let body = offers
                .iter()
                .find(|(candidate, _)| *candidate == media)
                .map(|(_, body)| *body)
                .unwrap_or_default();
            Response::builder()
                .status(status)
                .header("content-type", format!("{media}; charset=utf-8"))
                .header(header::VARY, "Accept, Accept-Encoding")
                // Every document that reaches this function is English prose:
                // the landing page (bilingual, English by default -- see
                // CONTENT_LANGUAGE_EN) and the four agent surfaces, which are
                // English on purpose and are not translated.
                .header(header::CONTENT_LANGUAGE, CONTENT_LANGUAGE_EN)
                .body(body.to_string())
                .unwrap()
        }
        crate::negotiate::Choice::NotAcceptable => {
            // RFC 9110 section 15.5.7 recommends naming the representations that
            // do exist, so the caller can retry with an `Accept` that works
            // instead of guessing.
            let available = types.join(", ");
            Response::builder()
                .status(StatusCode::NOT_ACCEPTABLE)
                .header("content-type", APPLICATION_JSON_UTF8)
                .header(header::VARY, "Accept, Accept-Encoding")
                // The Accept header is request-specific, so a shared cache must
                // not reuse this answer for the next caller.
                .header(header::CACHE_CONTROL, "no-store")
                .body(
                    json!({
                        "error": "No representation matches the Accept header",
                        "code": "not_acceptable",
                        "available": types,
                        "hint": format!(
                            "This resource is available as: {available}. \
                             Retry with an Accept header naming one of them, \
                             or omit Accept entirely to get the default."
                        ),
                    })
                    .to_string(),
                )
                .unwrap()
        }
    }
}

const TEXT_PLAIN_UTF8: &str = "text/plain; charset=utf-8";
const APPLICATION_JSON_UTF8: &str = "application/json; charset=utf-8";

/// Serve a compiled-in text document with an explicit content type.
fn text_surface(body: &'static str, content_type: &'static str) -> Response<String> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type)
        .body(body.to_string())
        .unwrap()
}

/// The four documents that exist in more than one media type.
///
/// Declared once, here, because each is referenced by its handler AND by the
/// negotiation tests -- two `include_str!` sites for the same file is how one
/// of them ends up serving a stale copy.
const LLMS_TXT: &str = include_str!("../static/llms.txt");
const INDEX_MD: &str = include_str!("../static/index.md");
const SKILL_MD: &str = include_str!("../static/skill.md");
const AUTH_MD: &str = include_str!("../static/auth.md");
const INDEX_HTML: &str = include_str!("../static/index.html");

/// The other three HTML pages, declared here for the same reason as the four
/// above: their handler and the i18n tests both read them, and two
/// `include_str!` sites for one file is how one of them ends up stale.
const BAZAAR_HTML: &str = include_str!("../static/bazaar.html");
const STATS_HTML: &str = include_str!("../static/stats.html");
const EVENTS_VIEWER_HTML: &str = include_str!("../static/events-viewer.html");

/// The MCP guide, in its two representations.
///
/// Both are read by their handler and by the tests, and the Markdown one is a
/// SEPARATE document rather than a rendering of the HTML: they are written for
/// different readers and only the HTML is bilingual. `/skill.md` section 10 is
/// the short version of the same material for an agent already calling
/// verify/settle.
const MCP_HTML: &str = include_str!("../static/mcp.html");
const MCP_MD: &str = include_str!("../static/mcp.md");

/// The network table. Its rows are NOT in this file: the page fetches
/// `/supported` in the browser and builds them, which is the whole reason it
/// exists. A network list typed into a document is wrong the first time a chain
/// ships and nobody remembers the document, and it stays wrong quietly.
const NETWORKS_HTML: &str = include_str!("../static/networks.html");

/// The x402 page: the two calls, with escrow and upto as sections rather than
/// as pages of their own -- they are extensions of `POST /settle`, not separate
/// endpoints, and giving each a page would say otherwise.
const X402_HTML: &str = include_str!("../static/x402.html");

/// `Content-Language` for every human page.
///
/// `en`, not `en, es`, and the difference is not pedantry. These pages carry
/// both languages in one document at one URL -- there is no `/es/` and no
/// `hreflang` -- but the bytes that render before the reader touches anything
/// are English, and this header describes the representation that was sent, not
/// the ones a click can reach. A cache or a crawler that keys on it must get
/// what it will actually read.
const CONTENT_LANGUAGE_EN: &str = "en";

/// One HTML page, with its content type and its declared language.
///
/// Every human page goes through here. Built as a helper rather than repeated
/// per handler because the failure is silent: a page added with a hand-written
/// `content-type` line and no `content-language` looks perfect in a browser.
fn html_page(body: &'static str) -> Response<String> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .header(header::CONTENT_LANGUAGE, CONTENT_LANGUAGE_EN)
        .body(body.to_string())
        .unwrap()
}

/// `GET /llms.txt`: the llmstxt.org map of this service.
///
/// Its bytes are Markdown and always were -- llmstxt.org specifies a Markdown
/// document. It stays `text/plain` by default because that is what every
/// existing consumer and the readiness checker expect from this path; an agent
/// that says `Accept: text/markdown` gets the same bytes correctly labelled.
#[instrument(skip_all)]
pub async fn get_llms_txt(headers: HeaderMap) -> impl IntoResponse {
    negotiated_surface(
        &headers,
        &[("text/plain", LLMS_TXT), ("text/markdown", LLMS_TXT)],
    )
}

/// `GET /llms-full.txt`: llms.txt, index.md, skill.md and auth.md in one file.
///
/// Generated by `scripts/build_llms_full.sh`; `llms_full_txt_is_in_sync` below
/// fails the build when the committed output stops matching its sources.
#[instrument(skip_all)]
pub async fn get_llms_full_txt() -> impl IntoResponse {
    text_surface(include_str!("../static/llms-full.txt"), TEXT_PLAIN_UTF8)
}

/// `GET /robots.txt`: crawler policy, with every AI crawler allowed explicitly.
#[instrument(skip_all)]
pub async fn get_robots_txt() -> impl IntoResponse {
    text_surface(include_str!("../static/robots.txt"), TEXT_PLAIN_UTF8)
}

/// `GET /sitemap.xml`: the pages a reader can land on.
#[instrument(skip_all)]
pub async fn get_sitemap_xml() -> impl IntoResponse {
    text_surface(
        include_str!("../static/sitemap.xml"),
        "application/xml; charset=utf-8",
    )
}

/// `GET /index.md`: the landing page in Markdown, for an agent that does not
/// want to render a 240 KB HTML monolith to learn what this is.
#[instrument(skip_all)]
pub async fn get_index_md(headers: HeaderMap) -> impl IntoResponse {
    negotiated_surface(&headers, &[("text/markdown", INDEX_MD)])
}

/// `GET /skill.md`: the operating manual for an agent calling verify/settle.
#[instrument(skip_all)]
pub async fn get_skill_md(headers: HeaderMap) -> impl IntoResponse {
    negotiated_surface(&headers, &[("text/markdown", SKILL_MD)])
}

/// `GET /auth.md`: how to authenticate here, which is: you do not.
#[instrument(skip_all)]
pub async fn get_auth_md(headers: HeaderMap) -> impl IntoResponse {
    negotiated_surface(&headers, &[("text/markdown", AUTH_MD)])
}

/// `GET /.well-known/ard.json`: the Agentic Resource Discovery catalog.
///
/// ARD v0.91 (<https://agenticresourcediscovery.org/spec>) is the one document
/// that names everything this host offers an agent -- the MCP server, the A2A
/// card, the skill, the OpenAPI and the llms.txt index -- so a consumer reads
/// one file instead of probing five paths. Section 5.1 fixes the path and makes
/// fetching it normative for a conforming consumer; the predecessor
/// `/.well-known/ai-catalog.json` is explicitly optional to consult, which is
/// why only this path is served.
///
/// Every entry carries the four terms section 4.2 requires -- `identifier`,
/// `displayName`, `type` and exactly one of `url`/`data` -- plus the
/// `representativeQueries` that section says separate an ARD entry from a bare
/// catalog listing. `the_ard_catalog_meets_the_spec` below re-checks all of
/// that, because the document is hand-written and the failure mode is silent:
/// a malformed entry is dropped by a registry without anyone being told.
///
/// NO `trustManifest`, DELIBERATELY. Section 4.5.1 binds `trustManifest.identity`
/// to the publisher domain in the URN and expects a registry to verify an
/// attestation issued by that domain. This host publishes no DID document and
/// no attestation, so a `did:web:` claim here would be an assertion nothing can
/// check. The term is optional and the scanner scores it as a bonus that never
/// costs points; an unverifiable claim would cost more than the bonus is worth.
#[instrument(skip_all)]
pub async fn get_ard() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/ard.json"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /workflows.json`: the state machines this facilitator drives.
#[instrument(skip_all)]
pub async fn get_workflows_json() -> impl IntoResponse {
    text_surface(
        include_str!("../static/workflows.json"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /.well-known/agent-card.json`: the A2A agent card.
#[instrument(skip_all)]
pub async fn get_agent_card() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/agent-card.json"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /.well-known/agent.json`: the older A2A card location.
///
/// Byte-identical to `/.well-known/agent-card.json` on purpose -- several
/// clients still look here first, and two cards that could disagree is worse
/// than one served twice.
#[instrument(skip_all)]
pub async fn get_agent_json_legacy() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/agent.json"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /.well-known/x402`: x402 discovery.
///
/// Declares `role: "facilitator"` and an empty `paidRoutes`, because that is
/// what is true: this service takes no fee and none of its routes answer 402.
#[instrument(skip_all)]
pub async fn get_x402_discovery() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/x402"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /.well-known/api-catalog`: the RFC 9727 linkset.
///
/// Served as `application/linkset+json`, the type RFC 9727 registers.
#[instrument(skip_all)]
pub async fn get_api_catalog() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/api-catalog"),
        "application/linkset+json; charset=utf-8",
    )
}

/// `GET /.well-known/oauth-protected-resource`: RFC 9728 metadata.
#[instrument(skip_all)]
pub async fn get_oauth_protected_resource() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/oauth-protected-resource"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /.well-known/agent-skills/index.json`: the downloadable-skills index.
#[instrument(skip_all)]
pub async fn get_agent_skills_index() -> impl IntoResponse {
    text_surface(
        include_str!("../static/.well-known/agent-skills/index.json"),
        APPLICATION_JSON_UTF8,
    )
}

/// `GET /.well-known/mcp/server-card.json`: where the MCP server lives, and
/// what it can do.
///
/// The document on disk carries no `serverInfo.version`, on purpose. The
/// release version is not a compile-time constant here -- `Cargo.toml` holds
/// the frozen `0.0.0` placeholder and the real number arrives at runtime as
/// `FACILITATOR_VERSION` (see `crate::version`). A number typed into the file
/// would be stale the first release nobody remembered to bump it, and a stale
/// version on a discovery card is worse than an absent one: a client that
/// caches the card believes it.
#[instrument(skip_all)]
pub async fn get_mcp_server_card() -> impl IntoResponse {
    text_surface(mcp_server_card(), APPLICATION_JSON_UTF8)
}

/// The served card: the static document with the running version stamped in.
///
/// Resolved once per process. Everything else about the card -- the endpoint,
/// the tool list, the transport -- stays in `static/`, where a reviewer can
/// read it without compiling anything.
fn mcp_server_card() -> &'static str {
    static CARD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CARD.get_or_init(|| {
        let mut doc: serde_json::Value =
            serde_json::from_str(include_str!("../static/.well-known/mcp/server-card.json"))
                .expect("static/.well-known/mcp/server-card.json must be valid JSON");
        doc["serverInfo"]["version"] = json!(crate::version::facilitator_version());
        serde_json::to_string_pretty(&doc).expect("a document that parsed must serialise")
    })
    .as_str()
}

pub fn routes<A>() -> Router<A>
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    Router::new()
        .route("/", get(get_root))
        .route("/bazaar", get(get_bazaar))
        .route("/networks", get(get_networks_page))
        .route("/x402", get(get_x402_page))
        .route("/events/live", get(get_events_viewer))
        .route("/stats", get(get_stats_page))
        // Escrow state query lives in secondary_read_routes() so it can carry
        // its own rate limit -- see that function for why.
        // ERC-8004 Registration endpoints (GET info only; gas-spending POST writes are
        // carved into erc8004_write_routes() so they can carry a strict rate limit -- audit 02)
        .route("/register", get(get_register_info))
        // ERC-8004 async registration status polling (P1). Deliberately on the
        // main router (not the strict write-governor) so frequent polling is
        // not rate-limited.
        .route("/register/status/{job_id}", get(get_register_status))
        // ERC-8004 Reputation endpoints (GET info only; the actual reputation
        // lookup lives in secondary_read_routes() so it can carry its own rate
        // limit -- see that function for why).
        .route("/feedback", get(get_feedback_info))
        // ERC-8004 Identity endpoints live in identity_read_routes() so they can
        // carry their own rate limit -- see that function for why.
        .route("/health", get(get_health))
        .route("/version", get(get_version))
        .route("/supported", get(get_supported::<A>))
        .route("/accepts", post(post_accepts::<A>))
        // /blacklist lives in secondary_read_routes() so it can carry its own
        // rate limit -- see that function for why.
        .route("/logo.png", get(get_logo))
        .route("/favicon.ico", get(get_favicon))
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
        .route("/xrpl.png", get(get_xrpl_logo))
        .route("/fogo.png", get(get_fogo_logo))
        .route("/algorand.png", get(get_algorand_logo))
        .route("/bsc.png", get(get_bsc_logo))
        .route("/sui.png", get(get_sui_logo))
        .route("/skale.png", get(get_skale_logo))
        .route("/scroll.png", get(get_scroll_logo))
        .route("/robinhood.png", get(get_robinhood_logo))
        .route("/usdc.png", get(get_usdc_logo))
        .route("/usdt.png", get(get_usdt_logo))
        .route("/eurc.png", get(get_eurc_logo))
        .route("/ausd.png", get(get_ausd_logo))
        .route("/pyusd.png", get(get_pyusd_logo))
        .route("/usdg.png", get(get_usdg_logo))
}

/// ERC-8004 gas-spending write routes, carved out so a strict rate limit can be
/// attached (audit 02): every `/register`, `/feedback`, `/feedback/revoke`,
/// `/feedback/response` is a real on-chain tx the facilitator EOA pays gas for.
/// Without a per-IP limit these are an unbounded gas-treasury drain.

/// Publish and record a failed operation, when the operator has opted in.
///
/// Off by default. Two guarantees preserved from the success path, because the
/// error branch is exactly where a system is already in trouble and least able
/// to absorb extra work: publishing is the LAST thing done, and it is
/// infallible — a failed payment cannot be made worse by someone watching.
#[allow(clippy::too_many_arguments)]
fn publish_failure(
    event_bus: &Arc<crate::events::EventBus>,
    tx_store: &Arc<dyn crate::transaction_store::TransactionStore>,
    kind: &'static str,
    requirements: &crate::types::PaymentRequirements,
    error_debug: &str,
) {
    if !event_bus.publish_failures() {
        return;
    }
    let category = failure_category(error_debug);
    let ts = crate::events::now_ms();
    event_bus.publish(crate::events::TrafficEvent {
        ts,
        kind,
        network: requirements.network.to_string(),
        ok: false,
        // No payer: on the error path we frequently do not have a trustworthy
        // one — a bad signature recovers to a meaningless address, and
        // publishing that would name an innocent party.
        payer: None,
        tx: None,
        amount: Some(requirements.max_amount_required.to_string()),
        asset: Some(requirements.asset.to_string()),
        resource: Some(requirements.resource.to_string()),
        pay_to: Some(requirements.pay_to.to_string()),
        description: Some(requirements.description.clone()),
        scheme: Some(requirements.scheme.to_string()),
        error: Some(category),
    });
    record_transaction(
        tx_store,
        crate::transaction_store::TransactionRecord {
            ts,
            kind: kind.into(),
            network: requirements.network.to_string(),
            ok: false,
            payer: None,
            tx: None,
            amount: Some(requirements.max_amount_required.to_string()),
            asset: Some(requirements.asset.to_string()),
            resource: Some(requirements.resource.to_string()),
            pay_to: Some(requirements.pay_to.to_string()),
            description: Some(requirements.description.clone()),
            scheme: Some(requirements.scheme.to_string()),
        },
    );
}

/// Map a facilitator error to a BOUNDED category for publication.
///
/// Classifies on the DEBUG VARIANT NAME, not on the message text. The handlers
/// are generic over `A::Error`, so the concrete enum is not in scope here — but
/// `{:?}` on any of these errors starts with the variant identifier, which is
/// far more stable than the human-readable message someone will inevitably
/// reword.
///
/// Deliberately lossy, and that is the point. The raw error carries addresses,
/// and `ContractCall` wraps the transport error verbatim — which on a bad day
/// is an RPC URL with the API key inside it. `src/redact.rs` exists because
/// exactly that leaked once. So the stream gets a closed set of strings that
/// cannot contain either.
///
/// The cost is real and worth stating: `rpc_error` does not say WHICH rpc. That
/// detail stays in the logs, where it is not world-readable.
fn failure_category(debug: &str) -> &'static str {
    let variant = debug.split(['(', ' ', '{']).next().unwrap_or("");
    match variant {
        "ContractCall" => "contract_revert",
        "InvalidSignature" => "invalid_signature",
        "InsufficientFunds" => "insufficient_funds",
        "InsufficientValue" => "insufficient_value",
        "InvalidTiming" => "invalid_timing",
        "BlockedAddress" => "blocked_address",
        "UnsupportedNetwork" => "unsupported_network",
        "NetworkMismatch" => "network_mismatch",
        "SchemeMismatch" => "scheme_mismatch",
        "ReceiverMismatch" => "receiver_mismatch",
        "InvalidAddress" => "invalid_address",
        "DecodingError" => "decoding_error",
        "ClockError" => "clock_error",
        // Anything unrecognised falls here rather than being echoed. A new
        // variant becomes "other" instead of leaking its payload.
        _ => "other",
    }
}

/// Did this failure come from the node we depend on, rather than the request?
///
/// The distinction decides the status code, and getting it wrong has a cost we
/// measured: while Celo's RPC was down, every settle there returned 400 — which
/// tells the caller "your request is malformed". Agents spent hours re-checking
/// signatures that were fine, because the only signal they got pointed at
/// themselves. Two of the three failure classes we see are not the caller's
/// fault at all.
///
/// The split is on the JSON-RPC error code, which is stable in a way the prose
/// is not:
///   * `code: 3` is an EVM execution revert — the chain ran the call and
///     rejected it. Bad signature, insufficient balance: genuinely about the
///     request, so 400 stays correct.
///   * `-32000`, `-32603`, `-32801` and transport errors are the node failing
///     to answer at all — pruned history, missing headers, retries exhausted.
///     Nothing in the request can fix those.
///   * `txpool is full` is matched by MESSAGE via
///     [`crate::chain::evm::is_mempool_full`], not by its `-32003` code — that
///     code is overloaded (see that function's doc comment) and a caller
///     retrying a genuine `-32003` payload rejection is not what this buys.
///
/// This retryable classification is only correct together with releasing the
/// nonce on the same condition (`evm.rs`'s `is_pre_broadcast_rejection`): a
/// `txpool is full` that keeps its nonce burned turns a client retry into a
/// faster nonce-gap wedge, not a cure. Deploy that fix first — see "Los fixes,
/// en el orden seguro" in
/// docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md.
///
/// Conservative by design: anything unrecognised keeps the old 400. A wrong 502
/// would tell a caller with a genuinely bad payload to go wait for us.
fn is_upstream_rpc_failure(debug: &str) -> bool {
    const NODE_CODES: [&str; 3] = ["-32000", "-32603", "-32801"];
    // An execution revert can also carry a node code in a nested transport
    // error, so the revert check wins: if the chain executed and rejected it,
    // that is an answer, not an outage.
    if debug.contains("execution reverted") {
        return false;
    }
    NODE_CODES.iter().any(|c| debug.contains(c))
        || debug.contains("Max retries exceeded")
        || debug.contains("Transport(")
        || crate::chain::evm::is_mempool_full(debug)
}

/// Persist one operation, off the request path.
///
/// Spawned rather than awaited: `record` makes two DynamoDB round trips, and
/// awaiting them would add tens of milliseconds to every settle response for a
/// write the payer does not care about. Failures are logged and dropped — the
/// store is an INDEX, and a payment that already happened must not be reported
/// as failed because a table was unreachable.
fn record_transaction(
    store: &Arc<dyn crate::transaction_store::TransactionStore>,
    record: crate::transaction_store::TransactionRecord,
) {
    let store = Arc::clone(store);
    tokio::spawn(async move {
        if let Err(e) = store.record(record).await {
            warn!(error = %e, "transaction not recorded; the payment itself is unaffected");
        }
    });
}

/// One resolved operation, in the shape both sinks need.
///
/// `/events` and the transaction store take the same facts and used to be fed
/// from two hand-written literals sitting next to each other, which is how they
/// drifted. Building this once and fanning out in [`emit_operation`] means a
/// field can no longer reach the stream but miss the index.
struct OperationDetail {
    kind: &'static str,
    network: String,
    ok: bool,
    payer: Option<String>,
    tx: Option<String>,
    amount: Option<String>,
    asset: Option<String>,
    resource: Option<String>,
    pay_to: Option<String>,
    description: Option<String>,
    scheme: Option<String>,
    /// `Some(category)` only for operations that ERRORED, never for one that
    /// merely resolved negative. The two are different events and conflating
    /// them is what makes a failure rate meaningless.
    error: Option<&'static str>,
}

/// Publish one operation to the live stream and the index.
///
/// Errored operations stay behind `X402_EVENTS_PUBLISH_FAILURES`, matching what
/// [`publish_failure`] already did and what the /stats page promises. Resolved
/// outcomes — including `isValid: false` — always go out: they are answers, not
/// failures, and suppressing them would make a legitimate rejection invisible.
fn emit_operation(
    event_bus: &Arc<crate::events::EventBus>,
    tx_store: &Arc<dyn crate::transaction_store::TransactionStore>,
    detail: OperationDetail,
) {
    if detail.error.is_some() && !event_bus.publish_failures() {
        return;
    }
    // A settlement that succeeded but whose asset we could not name still gets
    // recorded — dropping it would understate the operation count too — but it
    // must not do so quietly. Until this warning existed, an unresolved asset
    // produced a row indistinguishable from a legitimate one, and the gap grew
    // to a third of all settles before anyone noticed. Absence of a field is a
    // measurement failure and should read like one.
    if detail.ok && detail.kind == "settle" && detail.asset.is_none() {
        warn!(
            scheme = detail.scheme.as_deref().unwrap_or("unknown"),
            network = %detail.network,
            "settle recorded WITHOUT asset: volume for this operation will read as zero, \
             which is not the same as zero volume"
        );
    }
    let ts = crate::events::now_ms();
    event_bus.publish(crate::events::TrafficEvent {
        ts,
        kind: detail.kind,
        network: detail.network.clone(),
        ok: detail.ok,
        payer: detail.payer.clone(),
        tx: detail.tx.clone(),
        amount: detail.amount.clone(),
        asset: detail.asset.clone(),
        resource: detail.resource.clone(),
        pay_to: detail.pay_to.clone(),
        description: detail.description.clone(),
        scheme: detail.scheme.clone(),
        error: detail.error,
    });
    record_transaction(
        tx_store,
        crate::transaction_store::TransactionRecord {
            ts,
            kind: detail.kind.into(),
            network: detail.network,
            ok: detail.ok,
            payer: detail.payer,
            tx: detail.tx,
            amount: detail.amount,
            asset: detail.asset,
            resource: detail.resource,
            pay_to: detail.pay_to,
            description: detail.description,
            scheme: detail.scheme,
        },
    );
}

/// A resolved alternate-scheme branch: the response AND what it records.
///
/// The pairing is the whole point. Every branch in the alternate-scheme block
/// used to hand back a bare `Response` through an early `return`, while the
/// recorder sat further down the function — so fhe-transfer, upto and escrow
/// left no trace in `/events`, `/transactions` or `/api/stats`, and the numbers
/// looked healthy because the one scheme that DID record was the only one being
/// counted. Worse than incomplete: biased toward `exact`, silently.
///
/// Returning this struct makes the omission impossible to repeat. A new scheme
/// cannot be bolted on with a bare `return` — it does not typecheck until the
/// author decides what the operation records.
struct AltSchemeOutcome {
    response: Response,
    detail: OperationDetail,
}

/// Fields the alternate schemes carry, dug out of the raw envelope.
///
/// fhe-transfer, upto and escrow each have their own payload shape and none of
/// them parse into `PaymentRequirements`, so there is no typed path to these.
/// This probes the places the fields actually appear across the v1 and v2
/// envelopes and yields `None` where a field is genuinely absent.
///
/// Absent stays absent. Defaulting a missing asset or network to a plausible
/// value would write a wrong address into the index, where nothing downstream
/// could tell it apart from a measured one.
#[derive(Default)]
struct AltRequestFields {
    network: Option<String>,
    asset: Option<String>,
    amount: Option<String>,
    pay_to: Option<String>,
    resource: Option<String>,
}

/// Reduce whatever the caller wrote to the ONE name the rest of the system uses.
///
/// The alternate schemes read their network straight off the request, and x402
/// accepts three spellings of the same chain: the canonical name (`base`), the
/// CAIP-2 id (`eip155:8453`) and the inbound-only aliases `FromStr` tolerates.
/// `/api/stats` keys its rows on this string, so leaving them as sent split one
/// chain into several rows and inflated the "networks with activity" count —
/// observed in production the moment the first escrow verify was recorded.
///
/// An unrecognised value is passed through untouched rather than dropped: a
/// chain we cannot name still happened, and hiding it would be worse than
/// showing it under an odd label.
fn canonical_network_name(raw: &str) -> String {
    if let Some(n) = crate::network::Network::from_caip2(raw) {
        return n.to_string();
    }
    // Display, never Debug — Debug renders `SkaleBase`, which matches nothing.
    if let Ok(n) = raw.parse::<crate::network::Network>() {
        return n.to_string();
    }
    raw.to_string()
}

fn alt_request_fields(json_value: &serde_json::Value) -> AltRequestFields {
    // Candidate objects, most specific first: v1 requirements, v2 `accepted`
    // (object or first element of the array), and finally the envelope itself
    // for the top-level escrow shape.
    //
    // `payload` belongs in the owner list, not just `paymentPayload`. The
    // top-level escrow envelope nests its fields under a bare `payload`, and
    // leaving it out cost real accuracy: 84 of 317 recorded settles landed with
    // asset and amount null, which split each network into a second phantom row
    // with volume 0 — a figure that looked like a measurement and was an
    // artifact. Two payments from named agents in the swarm were verified to be
    // in that state.
    let payment_payload = json_value.get("paymentPayload");
    let bare_payload = json_value.get("payload");
    let mut candidates: Vec<&serde_json::Value> = Vec::new();
    for owner in [Some(json_value), payment_payload, bare_payload]
        .into_iter()
        .flatten()
    {
        for key in ["paymentRequirements", "accepted", "paymentInfo"] {
            if let Some(v) = owner.get(key) {
                match v {
                    serde_json::Value::Array(items) => candidates.extend(items.iter()),
                    other => candidates.push(other),
                }
            }
        }
    }
    candidates.push(json_value);
    for p in [payment_payload, bare_payload].into_iter().flatten() {
        candidates.push(p);
    }

    let first_str = |keys: &[&str]| -> Option<String> {
        for c in &candidates {
            for k in keys {
                match c.get(k) {
                    Some(serde_json::Value::String(s)) if !s.is_empty() => {
                        return Some(s.clone());
                    }
                    // v2 sends `resource` as an object; the url is the part
                    // worth indexing.
                    Some(serde_json::Value::Object(o)) => {
                        if let Some(serde_json::Value::String(s)) = o.get("url") {
                            return Some(s.clone());
                        }
                    }
                    // Amounts are u256-shaped and arrive as strings, but some
                    // clients send small ones as numbers.
                    Some(serde_json::Value::Number(n)) => return Some(n.to_string()),
                    _ => {}
                }
            }
        }
        None
    };

    AltRequestFields {
        network: first_str(&["network"]),
        asset: first_str(&["asset", "token"]),
        amount: first_str(&["maxAmountRequired", "amount", "maxAmount"]),
        pay_to: first_str(&["payTo", "receiver"]),
        resource: first_str(&["resource"]),
    }
}

/// The longest receipt wait any forwarded write can incur, in seconds.
///
/// The holder's wait is chosen PER NETWORK — Ethereum 900s, Base 90s, everything
/// else 30s — in [`evm_receipt_timeout`] and its twin in `chain::evm`. The
/// forwarding hop does not know which network it is carrying (the ERC-8004
/// routes never parse a network at this layer), so it has to budget for the
/// slowest one.
const LONGEST_RECEIPT_WAIT_SECS: u64 = 900;

/// How long a proxied write may take before this task gives up on the holder.
///
/// A settle waits for a receipt, so this has to clear the holder's own wait with
/// room to spare. Timing out the hop while the holder is still mining reports a
/// failure for a payment that then lands — the one outcome worse than refusing
/// outright, and the whole reason the forward exists.
///
/// This budgeted 60s + 30s until it was caught in review, which is a value that
/// appears nowhere in the receipt path: on Ethereum the holder waits up to 900s,
/// so the hop aborted at 90s with `forward_failed` while the transaction was
/// still perfectly alive, and on Base (90s) the promised margin was actually
/// negative once signing time is counted. Both carry real settle traffic.
///
/// `TX_RECEIPT_TIMEOUT_SECS` is read the same way the receipt path reads it: when
/// it is set it REPLACES the per-network default everywhere, so the hop needs
/// only that value plus the margin. When it is unset the per-network defaults
/// apply and the hop must cover the worst of them.
fn writer_forward_timeout() -> std::time::Duration {
    const FORWARD_MARGIN_SECS: u64 = 30;
    let receipt = std::env::var("TX_RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(LONGEST_RECEIPT_WAIT_SECS);
    std::time::Duration::from_secs(receipt.saturating_add(FORWARD_MARGIN_SECS))
}

/// The 503 a non-holder returns when it cannot hand the write to the holder.
fn writer_lease_unavailable(reason: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::RETRY_AFTER, "5")],
        Json(json!({
            "error": "this instance does not hold the EVM writer lease; retry",
            "reason": reason,
        })),
    )
        .into_response()
}

/// Route writes through the single instance that holds the EVM writer lease.
///
/// Every EVM write spends gas from the SAME shared EOA, and the nonce for it is
/// allocated in memory, so exactly one process may sign at a time. The settle
/// path has enforced that since the lease existed (`chain/evm.rs`); the
/// ERC-8004 handlers reach the chain through their own `contract.call().send()`
/// sites — around ten of them — and none passed through that gate. Gating the
/// ROUTER rather than the ten send sites is deliberate: a new write route is
/// covered the moment it is added here, with no per-call-site guard for a
/// future author to forget.
///
/// # Why this forwards instead of refusing
///
/// Refusing was right while "more than one task" meant "for about a minute per
/// deploy". On 2026-08-29 `min_capacity` went 1 -> 2 and the request-count
/// alarm took the service straight to 3, and refusing became a permanent
/// two-in-three failure rate on every EVM write — 582 settle-path rejections
/// and 132 ERC-8004 ones in a single six-hour window, with the lease never once
/// changing hands. Callers could not see the cause: they had a valid signature,
/// a funded signer, a passing `eth_call` simulation, and a 502.
///
/// Forwarding keeps the invariant exactly as it was — one process still
/// allocates every nonce — and removes the failure, because the task that
/// cannot sign hands the request to the one that can instead of dropping it.
///
/// # Bounded to one hop
///
/// A proxied request carries [`FORWARDED_HEADER`]. A task that receives one
/// while not holding the lease refuses rather than forwarding again, so a stale
/// endpoint cannot bounce a request between tasks until it times out.
///
/// Anything that goes wrong — no known holder, unreachable holder, a body too
/// large to buffer — falls back to the 503 this replaced, so the mechanism can
/// never be worse than the behaviour it supersedes.
async fn require_writer_lease(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if crate::writer_lease::is_writer() {
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();

    // Second hop: the holder we were sent to no longer holds the lease. Answer
    // rather than pass it on, so a stale endpoint cannot build a loop.
    if request
        .headers()
        .contains_key(crate::writer_lease::FORWARDED_HEADER)
    {
        warn!(
            path = %path,
            "refusing a forwarded EVM write: this instance is not the writer either"
        );
        return writer_lease_unavailable("forwarded_but_not_writer");
    }

    if !crate::writer_lease::forwarding_enabled() {
        warn!(path = %path, "rejecting EVM write: forwarding disabled");
        return writer_lease_unavailable("forwarding_disabled");
    }

    let Some(holder) = crate::writer_lease::holder_endpoint() else {
        warn!(path = %path, "rejecting EVM write: writer lease holder address unknown");
        return writer_lease_unavailable("holder_unknown");
    };

    match forward_to_writer(&holder, request).await {
        Ok(response) => response,
        Err(reason) => {
            warn!(
                path = %path,
                holder = %holder,
                reason = %reason,
                "forwarding an EVM write to the lease holder failed"
            );
            writer_lease_unavailable("forward_failed")
        }
    }
}

/// Whether a settle body targets an EVM chain, and therefore the shared EOA
/// whose nonce the writer lease serializes.
///
/// Biased toward `true`: only a network string that parses AND resolves to a
/// non-EVM family earns a `false`. Anything unreadable is treated as EVM and
/// forwarded, because the holder can serve every family while a non-holder
/// cannot serve EVM — guessing "not EVM" on an unparseable body would
/// resurrect the exact 503 this exists to remove.
///
/// Both protocol versions are covered: v1 spells the network `"base"`, v2
/// spells it `"eip155:8453"`, and either can appear on the payload or on the
/// requirements.
fn settle_body_targets_evm(body: &[u8]) -> bool {
    use std::str::FromStr;

    let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) else {
        return true;
    };

    let candidates = [
        json.pointer("/paymentPayload/network"),
        json.pointer("/paymentRequirements/network"),
        json.get("network"),
    ];

    for raw in candidates
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        let network = crate::network::Network::from_str(raw)
            .ok()
            .or_else(|| crate::network::Network::from_caip2(raw));
        if let Some(network) = network {
            return matches!(
                crate::network::NetworkFamily::from(network),
                crate::network::NetworkFamily::Evm
            );
        }
    }

    true
}

/// Route an EVM settle to the lease holder; serve every other family here.
///
/// `/settle` accounted for 582 of the 714 lease rejections measured in the six
/// hours before this landed — far more than the ERC-8004 routes — because it is
/// the busiest write on the service. It cannot simply reuse
/// [`require_writer_lease`], though: `/settle` also carries Solana, Stellar,
/// NEAR, Algorand, Sui and XRPL payments, which touch neither the EVM signer
/// nor its nonce. Forwarding those would funnel six chain families through one
/// task for no reason, trading a correctness bug for a capacity one.
///
/// So the decision is made on the body, and the body is put back afterwards:
/// buffering it here is the only way to read the network before the handler
/// does, and a handler that received an already-consumed body would fail in a
/// far more confusing way than the 503 this replaces.
async fn settle_writer_gate(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if crate::writer_lease::is_writer() {
        return next.run(request).await;
    }

    const MAX_SETTLE_BODY: usize = 1024 * 1024;
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_SETTLE_BODY).await {
        Ok(bytes) => bytes,
        // Let the handler produce the real error for an unreadable body rather
        // than inventing a lease-shaped one for it here.
        Err(e) => {
            warn!(error = %e, "could not buffer settle body for writer routing");
            return writer_lease_unavailable("body_unreadable");
        }
    };

    if !settle_body_targets_evm(&bytes) {
        // Non-EVM: this task is as good as any other. Reassemble and serve.
        let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
        return next.run(request).await;
    }

    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    require_writer_lease(request, next).await
}

/// Proxy one request to `holder` and return its response verbatim.
///
/// Deliberately transparent: same method, same path and query, same body, and
/// the upstream status and body handed straight back. A settle that the holder
/// rejects must look to the caller exactly like a settle this task rejected —
/// anything else would make the forwarding visible in the protocol.
async fn forward_to_writer(
    holder: &str,
    request: axum::extract::Request,
) -> Result<Response, String> {
    use axum::body::Body;

    let (parts, body) = request.into_parts();

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let url = format!("{}{}", holder.trim_end_matches('/'), path_and_query);

    // Writes on this service are small JSON documents. The cap is a guard
    // against buffering something unbounded in the proxy, not a protocol limit.
    const MAX_FORWARD_BODY: usize = 1024 * 1024;
    let bytes = axum::body::to_bytes(body, MAX_FORWARD_BODY)
        .await
        .map_err(|e| format!("could not buffer request body: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(writer_forward_timeout())
        .build()
        .map_err(|e| format!("could not build forwarding client: {e}"))?;

    let mut headers = parts.headers.clone();
    // Hop-by-hop and length headers describe THIS connection, not the next one;
    // reqwest sets its own. `host` would otherwise still name the ALB.
    for name in [
        axum::http::header::HOST,
        axum::http::header::CONTENT_LENGTH,
        axum::http::header::TRANSFER_ENCODING,
        axum::http::header::CONNECTION,
    ] {
        headers.remove(name);
    }
    headers.insert(
        axum::http::HeaderName::from_static(crate::writer_lease::FORWARDED_HEADER),
        axum::http::HeaderValue::from_static("1"),
    );

    let upstream = client
        .request(parts.method.clone(), &url)
        .headers(headers)
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("{e}"))?;

    let status = upstream.status();
    let mut response_headers = upstream.headers().clone();
    // Same reasoning in reverse: let axum frame the response it is about to
    // write, rather than replaying the upstream's framing.
    for name in [
        axum::http::header::CONTENT_LENGTH,
        axum::http::header::TRANSFER_ENCODING,
        axum::http::header::CONNECTION,
    ] {
        response_headers.remove(name);
    }

    let payload = upstream
        .bytes()
        .await
        .map_err(|e| format!("could not read holder response: {e}"))?;

    let mut response = Response::new(Body::from(payload));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    Ok(response)
}

/// Reject a `/feedback/revoke` call that does not carry the ERC-8004 admin
/// bearer token.
///
/// Why this route and not the others: `revokeFeedback` takes only
/// `agentId + feedbackIndex` and the registry authorises by `msg.sender`, which
/// is US. Every feedback the registry attributes to the facilitator wallet is
/// therefore revocable by whoever asks us to sign it, and until this gate
/// existed the only layer in front of the route was [`require_writer_lease`] —
/// a concurrency lease between ECS tasks, not authentication of the caller. An
/// anonymous POST could erase third-party reputation, permanently.
///
/// Middleware rather than a check inside the handler, deliberately: the handler
/// parses the body first, so a malformed body would answer 400 and reveal that
/// the route exists while the admin surface is supposed to be indistinguishable
/// from absent. Same reasoning as [`parse_admin_body`].
///
/// Fail-closed: with no `ERC8004_ADMIN_TOKEN` configured the route answers 404,
/// so deploying this turns revoke OFF until someone sets the secret on purpose.
async fn require_erc8004_admin(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(rejection) = admin_reject(admin_auth(request.headers(), ERC8004_ADMIN_TOKEN_VAR)) {
        warn!(
            path = %request.uri().path(),
            "rejecting ERC-8004 revoke: missing or invalid admin credentials"
        );
        return rejection;
    }
    next.run(request).await
}

pub fn erc8004_write_routes<A>() -> Router<A>
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider> + Sync,
{
    // `/feedback/revoke` carries a second gate the other writes do not; see
    // [`require_erc8004_admin`]. It keeps its own `Router` so both layers travel
    // with it and the public path does not change.
    //
    // Layer order matters: `.layer()` wraps, so the LAST one added runs FIRST.
    // Admin outermost means an unauthenticated caller gets the same answer
    // whether or not this instance holds the writer lease — a 503 that only
    // appears for the revoke path would leak that the route is live while the
    // admin surface is meant to look absent.
    let revoke = Router::new()
        .route("/feedback/revoke", post(post_revoke_feedback::<A>))
        .layer(axum::middleware::from_fn(require_writer_lease))
        .layer(axum::middleware::from_fn(require_erc8004_admin));

    Router::new()
        .route("/register", post(post_register::<A>))
        .route("/feedback", post(post_feedback::<A>))
        // Real authorship on SVM: the rater signs as `client`, we only pay.
        // `prepare` writes nothing, but it carries the same lease and rate limit
        // as `submit` because a prepared transaction is useless if `submit`
        // is shed anyway.
        .route(
            "/feedback/evm/prepare",
            post(post_prepare_relay_feedback::<A>),
        )
        .route(
            "/feedback/evm/submit",
            post(post_submit_relay_feedback::<A>),
        )
        .route(
            "/feedback/solana/prepare",
            post(post_prepare_solana_feedback::<A>),
        )
        .route(
            "/feedback/solana/submit",
            post(post_submit_solana_feedback::<A>),
        )
        .route("/feedback/response", post(post_append_response::<A>))
        .route(
            "/feedback/response/evm/prepare",
            post(post_prepare_relay_response::<A>),
        )
        .route(
            "/feedback/response/evm/submit",
            post(post_submit_relay_response::<A>),
        )
        // Applied here, not at the call sites, and not in main.rs: the gate
        // travels with the routes it protects. Merged AFTER this layer so the
        // revoke router keeps its own stack instead of being wrapped twice.
        .layer(axum::middleware::from_fn(require_writer_lease))
        .merge(revoke)
}

/// Discovery API routes for the Bazaar feature.
///
/// These routes are separate from the main facilitator routes because they use
/// a different state type (DiscoveryRegistry). The `/discovery/register` POST
/// is split out into [`discovery_register_routes`] so it can carry a stricter
/// rate limit (it triggers DNS lookups + outbound fetches against
/// attacker-supplied URLs).
/// Live traffic stream. Its own router + state so the generic `Facilitator` state is
/// untouched (same shape as `discovery_routes`).
pub fn events_routes() -> Router<Arc<crate::events::EventBus>> {
    Router::new().route("/events", get(get_events))
}

/// `GET /events` — Server-Sent Events, one message per facilitator operation.
///
/// SSE (not WebSocket) on purpose: the flow is one-way, `EventSource` reconnects on its
/// own in the browser, and a plain GET traverses the ALB without an upgrade handshake.
/// A lagging subscriber loses messages rather than applying back-pressure — the money
/// path must never wait on an observer.
pub async fn get_events(
    State(bus): State<Arc<crate::events::EventBus>>,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::wrappers::BroadcastStream;
    use tokio_stream::StreamExt as _;

    if !bus.enabled() {
        return (StatusCode::NOT_FOUND, "events stream disabled").into_response();
    }
    // Public, unauthenticated, and long-lived: admission is capped so a burst of
    // observers cannot exhaust the task that settles payments. Shedding here is a
    // plain 503 the client can retry — `EventSource` reconnects on its own.
    let Some(rx) = bus.try_subscribe() else {
        tracing::warn!(
            subscribers = bus.subscribers(),
            max = bus.max_subscribers(),
            "events stream at capacity, shedding subscriber"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::RETRY_AFTER, "30")],
            "events stream at capacity",
        )
            .into_response();
    };
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(ev) => Event::default()
            .event(ev.kind)
            .json_data(&ev)
            .ok()
            .map(Ok::<_, std::convert::Infallible>),
        // Lagged(n): this subscriber fell behind and n events were dropped for it.
        // Skip and keep the connection alive rather than tearing it down.
        Err(_) => None,
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

pub fn discovery_routes() -> Router<Arc<DiscoveryRegistry>> {
    Router::new()
        .route("/discovery/resources", get(get_discovery_resources))
        .route("/discovery/stats", get(get_discovery_stats))
        .route(
            "/discovery/attestation/{hash}",
            get(get_attestation_evidence),
        )
}

/// Admin routes for curating the Bazaar. Mounted behind the strict governor and
/// gated by `BAZAAR_ADMIN_TOKEN`; when that env var is unset every route here
/// answers 404 so the surface does not exist unless deliberately configured.
pub fn discovery_admin_routes() -> Router<Arc<DiscoveryRegistry>> {
    Router::new()
        .route(
            "/discovery/resources",
            axum::routing::delete(delete_discovery_resource),
        )
        .route("/discovery/admin/suppress", post(post_discovery_suppress))
        .route("/discovery/admin/release", post(post_discovery_release))
}

/// `GET /discovery/stats`: aggregate catalog metrics (60s cached).
#[instrument(skip_all)]
pub async fn get_discovery_stats(
    State(registry): State<Arc<DiscoveryRegistry>>,
) -> impl IntoResponse {
    (StatusCode::OK, Json(registry.stats().await))
}

/// Constant-time byte comparison. The length is not secret (and comparing it
/// first is what lets the loop stay fixed-width), but the contents are.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Outcome of admin bearer authentication.
pub(crate) enum AdminAuth {
    Ok,
    /// No token configured in the env var this surface reads — the admin
    /// surface is disabled.
    Disabled,
    Unauthorized,
}

/// The env var guarding the Bazaar curation admin routes.
const BAZAAR_ADMIN_TOKEN_VAR: &str = "BAZAAR_ADMIN_TOKEN";

/// The env var guarding `POST /feedback/revoke`.
///
/// Deliberately NOT `BAZAAR_ADMIN_TOKEN`: the blast radii are different. Leaking
/// the bazaar token hides or deletes a catalog listing; leaking this one signs
/// the destruction of third-party reputation on-chain, irreversibly. One
/// credential for both would mean the weaker surface sets the risk of the
/// stronger one.
const ERC8004_ADMIN_TOKEN_VAR: &str = "ERC8004_ADMIN_TOKEN";

/// Authenticate an admin request against the token in `env_var`. Never logs the
/// supplied credential.
///
/// The env var is a parameter rather than a constant because the admin surfaces
/// guarded by this function are not interchangeable — see
/// [`ERC8004_ADMIN_TOKEN_VAR`].
pub(crate) fn admin_auth(headers: &axum::http::HeaderMap, env_var: &str) -> AdminAuth {
    let Ok(expected) = std::env::var(env_var) else {
        return AdminAuth::Disabled;
    };
    if expected.is_empty() {
        return AdminAuth::Disabled;
    }
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
        AdminAuth::Ok
    } else {
        AdminAuth::Unauthorized
    }
}

/// Map a non-OK auth outcome to its response. 404 when disabled so the routes
/// are indistinguishable from not existing.
pub(crate) fn admin_reject(auth: AdminAuth) -> Option<Response<axum::body::Body>> {
    match auth {
        AdminAuth::Ok => None,
        AdminAuth::Disabled => {
            Some((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response())
        }
        AdminAuth::Unauthorized => Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid or missing admin credentials"})),
            )
                .into_response(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct AdminUrlQuery {
    /// Optional at the extractor level on purpose: a missing param must not
    /// produce a 400 before authentication, which would reveal that the admin
    /// route exists while the surface is supposed to be disabled.
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdminUrlBody {
    pub url: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `DELETE /discovery/resources?url=...`: permanently unregister a resource.
#[instrument(skip_all)]
pub async fn delete_discovery_resource(
    State(registry): State<Arc<DiscoveryRegistry>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AdminUrlQuery>,
) -> impl IntoResponse {
    if let Some(r) = admin_reject(admin_auth(&headers, BAZAAR_ADMIN_TOKEN_VAR)) {
        return r;
    }
    let Some(url) = q.url else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing required query parameter: url"})),
        )
            .into_response();
    };
    // Normalize before hitting the registry: keys are stored normalized, so a
    // raw query string would silently no-op.
    let key = match crate::discovery_security::canonical_url(&url) {
        Ok(c) => c.key,
        Err(_) => url.clone(),
    };
    match registry.unregister(&key).await {
        Ok(_) => {
            info!(url = %key, "Admin deleted discovery resource");
            (
                StatusCode::OK,
                Json(json!({"success": true, "url": key, "removed": true})),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "url": key, "error": "resource not found"})),
        )
            .into_response(),
    }
}

/// Parse an admin request body AFTER authentication. Taking raw bytes (rather
/// than the `Json` extractor) keeps a malformed body from returning 400 while
/// the admin surface is disabled — which would reveal that the route exists.
fn parse_admin_body(raw: &[u8]) -> Result<AdminUrlBody, Response<axum::body::Body>> {
    serde_json::from_slice::<AdminUrlBody>(raw).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("invalid request body: {e}")})),
        )
            .into_response()
    })
}

/// `POST /discovery/admin/suppress`: hide a resource without deleting it.
#[instrument(skip_all)]
pub async fn post_discovery_suppress(
    State(registry): State<Arc<DiscoveryRegistry>>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> impl IntoResponse {
    if let Some(r) = admin_reject(admin_auth(&headers, BAZAAR_ADMIN_TOKEN_VAR)) {
        return r;
    }
    let body = match parse_admin_body(&raw) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let changed = registry.suppress(&body.url).await;
    info!(url = %body.url, reason = ?body.reason, changed, "Admin suppressed resource");
    (
        StatusCode::OK,
        Json(json!({"success": true, "url": body.url, "suppressed": true, "changed": changed})),
    )
        .into_response()
}

/// `POST /discovery/admin/release`: un-suppress a resource.
#[instrument(skip_all)]
pub async fn post_discovery_release(
    State(registry): State<Arc<DiscoveryRegistry>>,
    headers: axum::http::HeaderMap,
    raw: axum::body::Bytes,
) -> impl IntoResponse {
    if let Some(r) = admin_reject(admin_auth(&headers, BAZAAR_ADMIN_TOKEN_VAR)) {
        return r;
    }
    let body = match parse_admin_body(&raw) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let changed = registry.release(&body.url).await;
    info!(url = %body.url, changed, "Admin released resource");
    (
        StatusCode::OK,
        Json(json!({"success": true, "url": body.url, "suppressed": false, "changed": changed})),
    )
        .into_response()
}

/// `GET /discovery/attestation/{hash}`: serve a hosted ERC-8004 attestation
/// evidence body (WS-E). Keyed by `sha256(url)` hex; only `[0-9a-f]{64}` keys
/// are accepted so a URL path segment can never be mapped to arbitrary content.
#[instrument(skip_all)]
pub async fn get_attestation_evidence(
    State(registry): State<Arc<DiscoveryRegistry>>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (StatusCode::BAD_REQUEST, "invalid evidence key").into_response();
    }
    match registry.get_evidence(&hash.to_ascii_lowercase()).await {
        Some(body) => (
            StatusCode::OK,
            [
                ("content-type", "application/json"),
                ("x-content-type-options", "nosniff"),
            ],
            String::from_utf8_lossy(&body).into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "evidence not found").into_response(),
    }
}

/// `POST /discovery/register` carved out into its own router so the strict
/// 5 req/min governor can be attached without affecting the read-only
/// `/discovery/resources` listing.
pub fn discovery_register_routes() -> Router<Arc<DiscoveryRegistry>> {
    Router::new().route("/discovery/register", post(post_discovery_register))
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

    /// Filter by liveness: alive|degraded|auth_gated|quarantined|unknown|unprobeable|any
    pub health: Option<String>,

    /// Filter by curated tier: first_party|vip|verified|listed
    pub tier: Option<String>,

    /// Free-text search over url / description / provider / category / tags.
    /// Capped at `MAX_SEARCH_LEN` characters.
    pub q: Option<String>,
}

/// Maximum accepted length of the `q` search parameter. The scan is O(items),
/// so an unbounded needle from an unauthenticated caller is a cheap CPU sink.
pub const MAX_SEARCH_LEN: usize = 128;

/// Every query parameter `GET /discovery/resources` understands.
///
/// Anything else is a 400. A parameter the server accepts and then ignores is
/// indistinguishable from a filter that matched everything: a caller passing
/// `?search=logs` got back the full unfiltered page and read it as a search
/// that matched the whole catalog, then filtered those hundred arbitrary rows
/// locally and called that the result. Silence is the bug; the rejection is
/// the fix.
pub const DISCOVERY_QUERY_PARAMS: &[&str] = &[
    "limit",
    "offset",
    "category",
    "network",
    "provider",
    "tag",
    "source",
    "sourceFacilitator",
    "health",
    "tier",
    "q",
];

/// Cap on how many rejected parameters are echoed back, and how much of each.
/// The names come from an unauthenticated caller and land in a response body.
const MAX_REPORTED_UNKNOWN: usize = 5;
const MAX_REPORTED_PARAM_LEN: usize = 64;

/// Map a rejected parameter to the one that does the job, when the intent is
/// obvious. Callers reach for `search` far more often than for `q`.
fn discovery_param_hint(unknown: &str) -> Option<&'static str> {
    match unknown.to_ascii_lowercase().as_str() {
        "search" | "query" | "text" | "term" | "keyword" | "filter" | "search_query" => Some("q"),
        "source_facilitator" | "facilitator" => Some("sourceFacilitator"),
        "page" | "skip" | "start" => Some("offset"),
        "count" | "size" | "limits" | "page_size" | "pagesize" | "per_page" | "perpage" => {
            Some("limit")
        }
        "status" | "liveness" | "alive" => Some("health"),
        "curation" | "tiers" | "label" => Some("tier"),
        "networks" | "chain" | "chain_id" | "chainid" => Some("network"),
        "tags" => Some("tag"),
        "categories" => Some("category"),
        _ => None,
    }
}

/// Collect the parameters in `raw` that the listing does not understand,
/// in the order they were sent and without repeats.
fn unknown_discovery_params(raw: &str) -> Vec<String> {
    let mut unknown: Vec<String> = Vec::new();
    for (key, _) in url::form_urlencoded::parse(raw.as_bytes()) {
        if DISCOVERY_QUERY_PARAMS.contains(&key.as_ref()) {
            continue;
        }
        if unknown.iter().any(|seen| seen == key.as_ref()) {
            continue;
        }
        unknown.push(key.into_owned());
    }
    unknown
}

/// Build the 400 body for rejected parameters.
fn unknown_params_response(unknown: &[String]) -> Response {
    let reported: Vec<String> = unknown
        .iter()
        .take(MAX_REPORTED_UNKNOWN)
        .map(|name| name.chars().take(MAX_REPORTED_PARAM_LEN).collect())
        .collect();

    let error = if reported.len() == 1 {
        format!("unknown query parameter: {}", reported[0])
    } else {
        format!("unknown query parameters: {}", reported.join(", "))
    };

    let mut body = json!({
        "error": error,
        "supported": DISCOVERY_QUERY_PARAMS,
    });

    // Only hint when it is unambiguous: one rejected parameter with one
    // obvious replacement.
    if let [only] = reported.as_slice() {
        if let Some(hint) = discovery_param_hint(only) {
            body["hint"] = json!(format!("did you mean {hint}?"));
        }
    }

    (StatusCode::BAD_REQUEST, Json(body)).into_response()
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
            && params.health.is_none()
            && params.tier.is_none()
            && params.q.is_none()
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
                health: params.health,
                tier: params.tier,
                q: params.q,
            })
        }
    }
}

/// `GET /discovery/resources`: List discoverable paid resources.
///
/// Supports pagination via `limit` and `offset` query parameters.
/// Supports filtering by `category`, `network`, `provider`, and `tag`.
///
/// Parameters outside `DISCOVERY_QUERY_PARAMS` are rejected with a 400 rather
/// than ignored, so a caller can tell a filter that matched everything apart
/// from a filter that was never applied.
///
/// # Example
/// ```text
/// GET /discovery/resources?limit=10&offset=0&category=finance&network=eip155:8453
/// ```
#[instrument(skip_all, fields(limit, offset, category, network))]
pub async fn get_discovery_resources(
    State(registry): State<Arc<DiscoveryRegistry>>,
    RawQuery(raw_query): RawQuery,
    Query(params): Query<DiscoveryQueryParams>,
) -> impl IntoResponse {
    if let Some(raw) = raw_query.as_deref() {
        let unknown = unknown_discovery_params(raw);
        if !unknown.is_empty() {
            warn!(
                unknown = ?unknown,
                "Discovery query rejected: unknown parameters"
            );
            return unknown_params_response(&unknown);
        }
    }

    debug!(
        limit = params.limit,
        offset = params.offset,
        category = ?params.category,
        network = ?params.network,
        "Discovery resources query"
    );

    // Bound the free-text needle: the scan is O(catalog) per request on a
    // public, unauthenticated route.
    if let Some(q) = params.q.as_deref() {
        if q.chars().count() > MAX_SEARCH_LEN {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("q must be at most {MAX_SEARCH_LEN} characters")
                })),
            )
                .into_response();
        }
    }

    let filters: Option<DiscoveryFilters> = params.clone().into();
    let response = registry.list(params.limit, params.offset, filters).await;

    info!(
        total = response.pagination.total,
        returned = response.items.len(),
        "Discovery query completed"
    );

    (StatusCode::OK, Json(response)).into_response()
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
pub async fn get_root(headers: HeaderMap) -> impl IntoResponse {
    // HTML first, so it stays the default: a browser, a `curl` with no Accept,
    // and anything sending `*/*` all still get the landing page. Markdown is
    // the same content an agent would otherwise have to render 240 KB of HTML
    // to reach -- it is `/index.md`, byte for byte, so the two cannot drift.
    negotiated_surface(
        &headers,
        &[("text/html", INDEX_HTML), ("text/markdown", INDEX_MD)],
    )
}

/// `GET /events/live`: the live traffic viewer.
///
/// Served from the binary rather than a local file because Chrome and Brave
/// treat `file://` as an opaque origin and block its cross-origin requests
/// regardless of CORS headers — a page opened by double-click could never
/// connect to the stream.
#[instrument(skip_all)]
pub async fn get_events_viewer() -> impl IntoResponse {
    html_page(EVENTS_VIEWER_HTML)
}

/// `GET /stats`: aggregated metrics, human-readable.
///
/// Its own page rather than another section of the landing page: the landing
/// page is already a monolith, and metrics are read by someone asking a
/// different question than someone evaluating the service.
#[instrument(skip_all)]
pub async fn get_stats_page() -> impl IntoResponse {
    html_page(STATS_HTML)
}

/// Alias for `get_root` to match main.rs routing.
pub async fn get_index(headers: HeaderMap) -> impl IntoResponse {
    get_root(headers).await
}

/// The 405 `GET /mcp` answers a caller that is an MCP client, not a reader.
///
/// rmcp answers 405 for GET when sessions are off, but with a `text/plain`
/// body and no `content-type` at all. A scanner grades a surface on its
/// content type as much as its status, so this route is served by us: same
/// 405, same `Allow: POST`, but a body a machine can read.
///
/// It lives here rather than in `mcp.rs` because `mod mcp` is declared only in
/// `main.rs`: the library compiles `handlers.rs` and would not find it.
pub fn mcp_get_not_allowed() -> Response<String> {
    let body = json!({
        "error": "GET is not supported on /mcp",
        "reason": "This MCP server runs stateless: there is no server-initiated SSE \
                   stream to open. Send JSON-RPC over POST instead.",
        "transport": "streamable-http",
        "method": "POST",
        "humanGuide": "https://facilitator.ultravioletadao.xyz/mcp",
        "serverCard": "https://facilitator.ultravioletadao.xyz/.well-known/mcp/server-card.json"
    });
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::ALLOW, "POST")
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(body.to_string())
        .expect("a constant response builds")
}

/// `GET /mcp`: the MCP guide for a person, without taking the path from the
/// MCP server that lives on the same URL under `POST`.
///
/// **The `Accept` decides who is asking, and the two answers are different on
/// purpose.** The Streamable HTTP transport sends
/// `application/json, text/event-stream` on every request it makes, so a caller
/// arriving here with that header is an MCP client that used the wrong method,
/// and what it needs is [`mcp_get_not_allowed`] -- the 405 naming
/// POST -- not two hundred lines of HTML it cannot parse. Everyone else gets
/// the page, and `Accept: text/markdown` gets the same guide as Markdown.
///
/// Serving a page here also replaces the old behaviour, where the ONLY thing at
/// `/mcp` for a person who clicked the link in a config file was a 405.
#[instrument(skip_all)]
pub async fn get_mcp_page(headers: HeaderMap) -> Response<String> {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let asks_for_html = accept.contains("text/html");
    if accept.contains("text/event-stream") || (accept.contains("application/json") && !asks_for_html)
    {
        return mcp_get_not_allowed();
    }
    // HTML first, so it stays the default for a browser, for `curl` with no
    // Accept, and for anything sending `*/*`.
    let mut response =
        negotiated_surface(&headers, &[("text/html", MCP_HTML), ("text/markdown", MCP_MD)]);
    // The Markdown is English-only; the HTML is English by default. Both are
    // `en` -- see CONTENT_LANGUAGE_EN.
    response.headers_mut().insert(
        header::CONTENT_LANGUAGE,
        header::HeaderValue::from_static(CONTENT_LANGUAGE_EN),
    );
    response
}

/// `GET /networks`: every network and scheme, read live from `/supported`.
///
/// Its own page because the landing page used to carry the whole wall -- two
/// tabs, forty cards, every balance -- which made the first screen of the site
/// a list of chains instead of an answer to what the service does. The wall was
/// also hand-written, so it drifted from `/supported` by construction.
#[instrument(skip_all)]
pub async fn get_networks_page() -> impl IntoResponse {
    html_page(NETWORKS_HTML)
}

/// `GET /x402`: what this facilitator does, for a person deciding whether to
/// use it -- including the counter of settlements that FAILED.
///
/// Publishing only the successes would be advertising, not reporting, and the
/// endpoint that produces both numbers ships its own caveat with them; the page
/// prints that caveat verbatim rather than paraphrasing it.
#[instrument(skip_all)]
pub async fn get_x402_page() -> impl IntoResponse {
    html_page(X402_HTML)
}

/// `GET /bazaar`: Returns the curated Bazaar resource explorer (WS-D).
#[instrument(skip_all)]
pub async fn get_bazaar() -> impl IntoResponse {
    html_page(BAZAAR_HTML)
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

/// `GET /xrpl.png`: Returns XRPL (XRP Ledger) logo.
#[instrument(skip_all)]
pub async fn get_xrpl_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/xrpl.png");
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

/// `GET /robinhood.png`: Returns Robinhood Chain logo.
pub async fn get_robinhood_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/robinhood.png");
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

/// `GET /usdg.png`: Returns USDG (Global Dollar) stablecoin logo.
pub async fn get_usdg_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/usdg.png");
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
            // Convert v1 response to v2 with the extensions this deployment
            // actually serves. `durable-evidence` is advertised only when DX402
            // is configured -- announcing an extension whose routes 404 would
            // make integrators build against a capability that is not there.
            let mut extensions = vec!["bazaar".to_string()];
            // `is_serviceable`, not `enabled`: the flag can be on while the
            // bucket is missing, in which case the service is never built and
            // every /dx402 route 404s. See `Dx402Config::is_serviceable`.
            if crate::dx402::Dx402Config::from_env().is_serviceable() {
                extensions.push(crate::dx402::EXTENSION_KEY.to_string());
            }
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
        let extra_json = kind
            .extra
            .as_ref()
            .and_then(|e| serde_json::to_value(e).ok());
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
                let req_extra = enriched_req.get("extra").cloned().unwrap_or(json!({}));
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
        "version": crate::version::facilitator_version()
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
    Extension(event_bus): Extension<Arc<crate::events::EventBus>>,
    Extension(tx_store): Extension<Arc<dyn crate::transaction_store::TransactionStore>>,
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
    // Alternate schemes resolve inside this block and yield an outcome instead
    // of returning a response straight out of the handler. That indirection is
    // what makes them countable: every branch here now carries the record it
    // produces (see `AltSchemeOutcome`), so fhe-transfer, upto and escrow reach
    // `/events` and the index like `exact` always has.
    let alt_outcome: Option<AltSchemeOutcome> = async {
        let json_value = serde_json::from_str::<serde_json::Value>(body_str).ok()?;
        let fields = alt_request_fields(&json_value);
        let detail = |ok: bool, scheme: Option<&str>, error: Option<&'static str>| OperationDetail {
            kind: "verify",
            // "unknown" rather than a guess. These payloads do not always name
            // a network, and an honest "unknown" bucket in /stats is worth more
            // than a plausible-looking wrong one.
            network: fields
                .network
                .as_deref()
                .map(canonical_network_name)
                .unwrap_or_else(|| "unknown".to_string()),
            ok,
            payer: None,
            // A verify settles nothing, so there is no hash to carry.
            tx: None,
            amount: fields.amount.clone(),
            asset: fields.asset.clone(),
            resource: fields.resource.clone(),
            pay_to: fields.pay_to.clone(),
            description: None,
            scheme: scheme.map(|s| s.to_string()),
            error,
        };

        // Detect scheme from paymentPayload.scheme (v1) or paymentPayload.accepted.scheme (v2)
        let scheme = json_value.get("paymentPayload").and_then(|pp| {
            pp.get("scheme").and_then(|s| s.as_str()).or_else(|| {
                pp.get("accepted")
                    .and_then(|a| a.get("scheme"))
                    .and_then(|s| s.as_str())
            })
        });

        if scheme == Some("fhe-transfer") {
            info!("Detected fhe-transfer scheme, routing to Zama Lambda facilitator");

            match FHE_PROXY.verify(&json_value).await {
                Ok(fhe_response) => {
                    info!(
                        is_valid = fhe_response.is_valid,
                        "FHE verification complete"
                    );
                    let mut d = detail(fhe_response.is_valid, scheme, None);
                    d.payer = fhe_response.payer.clone();
                    return Some(AltSchemeOutcome {
                        response: (StatusCode::OK, Json(fhe_response)).into_response(),
                        detail: d,
                    });
                }
                Err(e) => {
                    error!(error = %e, "FHE verification failed");
                    return Some(AltSchemeOutcome {
                        response: (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({
                                "isValid": false,
                                "invalidReason": format!("FHE facilitator error: {}", e)
                            })),
                        )
                            .into_response(),
                        detail: detail(false, scheme, Some("fhe_error")),
                    });
                }
            }
        }

        // Check for upto scheme (Permit2-based variable amount settlement)
        if scheme == Some("upto") {
            if !crate::upto::is_enabled() {
                warn!("Upto scheme verify requested but ENABLE_UPTO is not set to true");
                return Some(AltSchemeOutcome {
                    response: (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "isValid": false,
                            "invalidReason": "Upto scheme is disabled. Set ENABLE_UPTO=true to enable."
                        })),
                    )
                        .into_response(),
                    detail: detail(false, scheme, Some("scheme_disabled")),
                });
            }

            info!("Detected upto scheme, routing to Permit2 verification");

            match crate::upto::verify_upto(body_str, &facilitator).await {
                Ok(response) => {
                    info!("Upto verification complete");
                    // Untyped response: read the verdict off the wire, and treat
                    // a missing `isValid` as not valid.
                    let is_valid = response
                        .get("isValid")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mut d = detail(is_valid, scheme, None);
                    d.payer = response
                        .get("payer")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Some(AltSchemeOutcome {
                        response: (StatusCode::OK, Json(response)).into_response(),
                        detail: d,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Upto verification failed");
                    return Some(AltSchemeOutcome {
                        response: (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "isValid": false,
                                "invalidReason": format!("Upto verification error: {}", e)
                            })),
                        )
                            .into_response(),
                        detail: detail(false, scheme, Some("upto_error")),
                    });
                }
            }
        }

        // Check for escrow/commerce scheme (x402r PaymentOperator), either
        // nested in the payload or declared at the top level.
        let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());
        let escrow_scheme = if crate::payment_operator::is_escrow_scheme(scheme) {
            scheme
        } else if crate::payment_operator::is_escrow_scheme(top_level_scheme) {
            top_level_scheme
        } else {
            None
        };
        if escrow_scheme.is_some() {
            if !crate::payment_operator::is_enabled() {
                warn!(
                    "Escrow scheme verify requested but ENABLE_PAYMENT_OPERATOR is not set to true"
                );
                return Some(AltSchemeOutcome {
                    response: (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "isValid": false,
                            "invalidReason": "Escrow scheme is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable."
                        })),
                    )
                        .into_response(),
                    detail: detail(false, escrow_scheme, Some("scheme_disabled")),
                });
            }

            info!("Detected escrow scheme, routing to PaymentOperator verification");

            match crate::payment_operator::verify_escrow(body_str, &facilitator).await {
                Ok(response) => {
                    info!("Escrow verification complete");
                    // verify_escrow hands back an untyped Value, so the verdict
                    // is read from the wire field. Absent reads as not-valid:
                    // a response that does not say it is valid is not one.
                    let is_valid = response
                        .get("isValid")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mut d = detail(is_valid, escrow_scheme, None);
                    d.payer = response
                        .get("payer")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Some(AltSchemeOutcome {
                        response: (StatusCode::OK, Json(response)).into_response(),
                        detail: d,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Escrow verification failed");
                    return Some(AltSchemeOutcome {
                        response: (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "isValid": false,
                                "invalidReason": format!("Escrow verification error: {}", e)
                            })),
                        )
                            .into_response(),
                        detail: detail(false, escrow_scheme, Some("escrow_error")),
                    });
                }
            }
        }

        None
    }
    .await;

    if let Some(outcome) = alt_outcome {
        emit_operation(&event_bus, &tx_store, outcome.detail);
        return outcome.response;
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
                    // Log first 2000 chars of the payload for debugging.
                    // SECURITY (audit 4B.1): use a char-safe boundary -- `&body_str[..2000]`
                    // panics when byte 2000 falls inside a multi-byte UTF-8 char, which an
                    // attacker can trigger with a crafted body (DoS on the error path).
                    let truncated = if body_str.len() > 2000 {
                        let end = body_str
                            .char_indices()
                            .map(|(i, _)| i)
                            .take_while(|&i| i <= 2000)
                            .last()
                            .unwrap_or(0);
                        format!("{}... (truncated)", &body_str[..end])
                    } else {
                        body_str.to_string()
                    };
                    warn!("Received payload: {}", truncated);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": format!("Failed to deserialize VerifyRequest: {}", e),
                            "code": "invalid_request_body",
                            "hint": "The body must be a JSON object with \
                                     `paymentPayload` and `paymentRequirements`. \
                                     Both the x402 v1 shape (\"network\": \"base\") \
                                     and the v2 CAIP-2 shape (\"network\": \
                                     \"eip155:8453\") are accepted. Worked examples: \
                                     https://facilitator.ultravioletadao.xyz/skill.md",
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
        Ok(valid_response) => {
            // Live traffic stream — after the verification resolved, best-effort and
            // infallible by design (see src/events.rs). A verify carries no tx hash:
            // nothing has been settled yet, and inventing one would make the stream lie.
            let (ok, payer) = match &valid_response {
                VerifyResponse::Valid { payer } => (true, Some(payer.to_string())),
                VerifyResponse::Invalid { payer, .. } => {
                    (false, payer.as_ref().map(|p| p.to_string()))
                }
            };
            let payer_for_record = payer.clone();
            event_bus.publish(crate::events::TrafficEvent {
                ts: crate::events::now_ms(),
                kind: "verify",
                network: v1_request.payment_requirements.network.to_string(),
                ok,
                payer,
                tx: None,
                amount: Some(
                    v1_request
                        .payment_requirements
                        .max_amount_required
                        .to_string(),
                ),
                asset: Some(v1_request.payment_requirements.asset.to_string()),
                resource: Some(v1_request.payment_requirements.resource.to_string()),
                pay_to: Some(v1_request.payment_requirements.pay_to.to_string()),
                description: Some(v1_request.payment_requirements.description.clone()),
                scheme: Some(v1_request.payment_requirements.scheme.to_string()),
                error: None,
            });
            record_transaction(
                &tx_store,
                crate::transaction_store::TransactionRecord {
                    ts: crate::events::now_ms(),
                    kind: "verify".into(),
                    network: v1_request.payment_requirements.network.to_string(),
                    ok,
                    payer: payer_for_record,
                    tx: None,
                    amount: Some(
                        v1_request
                            .payment_requirements
                            .max_amount_required
                            .to_string(),
                    ),
                    asset: Some(v1_request.payment_requirements.asset.to_string()),
                    resource: Some(v1_request.payment_requirements.resource.to_string()),
                    pay_to: Some(v1_request.payment_requirements.pay_to.to_string()),
                    description: Some(v1_request.payment_requirements.description.clone()),
                    scheme: Some(v1_request.payment_requirements.scheme.to_string()),
                },
            );
            (StatusCode::OK, Json(valid_response)).into_response()
        }
        Err(error) => {
            tracing::warn!(
                error = ?error,
                version = ?version,
                body = %serde_json::to_string(&v1_request).unwrap_or_else(|_| "<can-not-serialize>".to_string()),
                "Verification failed"
            );
            publish_failure(
                &event_bus,
                &tx_store,
                "verify",
                &v1_request.payment_requirements,
                &format!("{error:?}"),
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
    Extension(event_bus): Extension<Arc<crate::events::EventBus>>,
    Extension(tx_store): Extension<Arc<dyn crate::transaction_store::TransactionStore>>,
    headers: HeaderMap,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // F4: idempotency store lives in a process-global OnceCell. The read
    // and write helpers (`lookup_record` / `store_record`) are dispatched
    // through `tokio::spawn` so the outer handler future is `Send` — calling
    // the `#[async_trait]` method on `Arc<dyn IdempotencyStore + Send + Sync>`
    // directly from this generic handler tripped axum's Handler trait
    // elaboration (the dyn-trait future's lifetime couldn't be proved
    // `Send + 'static` through the routing layer, even with explicit bounds).
    // F4: extract the Idempotency-Key header (Stripe-style). If present, we
    // hash the canonical request body so we can detect "same key + same body"
    // replays (return cached) vs "same key + different body" (refuse with 409).
    // Header lookup is case-insensitive in HeaderMap. The actual hash is
    // computed below against `body_str` so it normalises across the v1 raw
    // body and the v2 base64-decoded PAYMENT-SIGNATURE transports.
    let idempotency_key: Option<String> = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
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

    // F4: idempotency cache lookup against canonical body bytes. The hash
    // is sha256(body_str) which intentionally matches across v1 raw body
    // and v2 PAYMENT-SIGNATURE transports — a retry with the same logical
    // request returns the cached response, while a same-key reuse with a
    // different body is refused.
    let request_hash: Option<String> = idempotency_key
        .as_ref()
        .map(|_| hash_request_body(body_str.as_bytes()));
    let lookup_key = idempotency_key.clone();
    let lookup_hash = request_hash.clone();
    if let (Some(key), Some(hash)) = (lookup_key, lookup_hash) {
        let key_for_lookup = key.clone();
        let lookup_result =
            tokio::spawn(
                async move { crate::idempotency_store::lookup_record(key_for_lookup).await },
            )
            .await
            .unwrap_or_else(|e| {
                Err(crate::idempotency_store::IdempotencyStoreError::ReadError(
                    format!("idempotency lookup task panicked: {e}"),
                ))
            });
        match lookup_result {
            Ok(Some(record)) if record.request_hash == hash => {
                info!(idempotency_key = %key, "Serving cached /settle response");
                let parsed: SettleResponse = match serde_json::from_str(&record.response_json) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            idempotency_key = %key,
                            error = %e,
                            "Cached idempotency record could not be parsed; re-running settle"
                        );
                        // Fall through to normal settle path. We can't return
                        // here because the outer `if let` only runs once.
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"error": "idempotency_cache_corrupt"})),
                        )
                            .into_response();
                    }
                };
                return (StatusCode::OK, Json(parsed)).into_response();
            }
            Ok(Some(_)) => {
                let correlation_id = uuid::Uuid::new_v4();
                warn!(
                    %correlation_id,
                    idempotency_key = %key,
                    "Idempotency-Key reused with different body"
                );
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "idempotency_key_conflict",
                        "correlation_id": correlation_id.to_string(),
                    })),
                )
                    .into_response();
            }
            Ok(None) => {
                debug!(idempotency_key = %key, "Idempotency cache miss");
            }
            Err(e) => {
                // Fail closed — refusing to settle is safer than risking a
                // double-spend window when the cache is unavailable.
                let correlation_id = uuid::Uuid::new_v4();
                error!(
                    %correlation_id,
                    idempotency_key = %key,
                    error = %e,
                    "Idempotency store unavailable; refusing to settle"
                );
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "idempotency_store_unavailable",
                        "correlation_id": correlation_id.to_string(),
                    })),
                )
                    .into_response();
            }
        }
    }

    debug!("=== SETTLE REQUEST DEBUG ===");
    debug!("Raw JSON body: {}", body_str);

    // Check for special schemes BEFORE trying to parse as standard types
    // These schemes may have different payload structures that don't match standard x402 types
    // Alternate settlement schemes resolve inside this block and yield an
    // outcome instead of returning straight out of the handler.
    //
    // This is the fix for a real, measured hole: escrow settlements were
    // succeeding on-chain and leaving NO trace in `/events`, `/transactions` or
    // `/api/stats`, because each branch below returned 200 before reaching the
    // recorder further down. `/api/stats` was therefore not just incomplete but
    // biased — it counted `exact` and nothing else, while escrow was the scheme
    // actually carrying traffic.
    let alt_outcome: Option<AltSchemeOutcome> = async {
        let json_value = serde_json::from_str::<serde_json::Value>(body_str).ok()?;
        let fields = alt_request_fields(&json_value);
        let detail = |ok: bool, scheme: Option<&str>, error: Option<&'static str>| OperationDetail {
            kind: "settle",
            network: fields
                .network
                .as_deref()
                .map(canonical_network_name)
                .unwrap_or_else(|| "unknown".to_string()),
            ok,
            payer: None,
            tx: None,
            amount: fields.amount.clone(),
            asset: fields.asset.clone(),
            resource: fields.resource.clone(),
            pay_to: fields.pay_to.clone(),
            description: None,
            scheme: scheme.map(|s| s.to_string()),
            error,
        };

        // Detect scheme from paymentPayload.scheme (v1) or paymentPayload.accepted.scheme (v2)
        let scheme = json_value.get("paymentPayload").and_then(|pp| {
            pp.get("scheme").and_then(|s| s.as_str()).or_else(|| {
                pp.get("accepted")
                    .and_then(|a| a.get("scheme"))
                    .and_then(|s| s.as_str())
            })
        });

        if scheme == Some("fhe-transfer") {
            info!("Detected fhe-transfer scheme, routing settle to Zama Lambda facilitator");

            match FHE_PROXY.settle(&json_value).await {
                Ok(fhe_response) => {
                    info!("FHE settlement complete");
                    // Untyped response: read the verdict off the wire, and treat
                    // a missing `success` as not successful.
                    let ok = fhe_response
                        .get("success")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mut d = detail(ok, scheme, None);
                    d.tx = fhe_response
                        .get("transaction")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    d.payer = fhe_response
                        .get("payer")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Some(AltSchemeOutcome {
                        response: (StatusCode::OK, Json(fhe_response)).into_response(),
                        detail: d,
                    });
                }
                Err(e) => {
                    error!(error = %e, "FHE settlement failed");
                    return Some(AltSchemeOutcome {
                        response: (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({
                                "success": false,
                                "errorReason": format!("FHE facilitator error: {}", e)
                            })),
                        )
                            .into_response(),
                        detail: detail(false, scheme, Some("fhe_error")),
                    });
                }
            }
        }

        // Check for upto scheme (Permit2-based variable amount settlement)
        if scheme == Some("upto") {
            if !crate::upto::is_enabled() {
                warn!("Upto scheme settle requested but ENABLE_UPTO is not set to true");
                return Some(AltSchemeOutcome {
                    response: (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": "Upto scheme is disabled. Set ENABLE_UPTO=true to enable."
                        })),
                    )
                        .into_response(),
                    detail: detail(false, scheme, Some("scheme_disabled")),
                });
            }

            info!("Detected upto scheme, routing to Permit2 settlement");

            match crate::upto::settle_upto(body_str, &facilitator).await {
                Ok(upto_response) => {
                    info!("Upto settlement complete");
                    let mut d = detail(upto_response.success, scheme, None);
                    d.network = canonical_network_name(&upto_response.network);
                    d.tx = Some(upto_response.transaction.clone());
                    d.payer = upto_response.payer.clone();
                    // upto settles a VARIABLE amount, so the figure that matters
                    // is what was actually pulled, not the ceiling requested.
                    d.amount = Some(upto_response.amount.clone());
                    return Some(AltSchemeOutcome {
                        response: (StatusCode::OK, Json(upto_response)).into_response(),
                        detail: d,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Upto settlement failed");
                    return Some(AltSchemeOutcome {
                        response: (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "success": false,
                                "errorReason": format!("Upto scheme error: {}", e)
                            })),
                        )
                            .into_response(),
                        detail: detail(false, scheme, Some("upto_error")),
                    });
                }
            }
        }

        // Check for escrow/commerce scheme (x402r PaymentOperator), nested in
        // paymentPayload (v2) or declared at the top level.
        let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());
        let escrow_scheme = if crate::payment_operator::is_escrow_scheme(scheme) {
            scheme
        } else if crate::payment_operator::is_escrow_scheme(top_level_scheme) {
            top_level_scheme
        } else {
            None
        };
        if escrow_scheme.is_some() {
            if !crate::payment_operator::is_enabled() {
                warn!("Escrow scheme settlement requested but ENABLE_PAYMENT_OPERATOR is not set to true");
                return Some(AltSchemeOutcome {
                    response: (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": "Escrow scheme settlement is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable."
                        })),
                    )
                        .into_response(),
                    detail: detail(false, escrow_scheme, Some("scheme_disabled")),
                });
            }

            info!("Detected escrow scheme, routing to PaymentOperator settlement");

            match crate::payment_operator::settle_escrow(body_str, &facilitator).await {
                Ok(escrow_response) => {
                    info!("Escrow scheme settlement complete");
                    let mut d = detail(escrow_response.success, escrow_scheme, None);
                    // Display, not Debug: Debug renders `SkaleBase`, which
                    // matches no network name any consumer knows.
                    d.network = escrow_response.network.to_string();
                    d.tx = escrow_response.transaction.as_ref().map(|t| t.to_string());
                    d.payer = Some(escrow_response.payer.to_string());
                    return Some(AltSchemeOutcome {
                        response: (StatusCode::OK, Json(escrow_response)).into_response(),
                        detail: d,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Escrow scheme settlement failed");
                    // A node that cannot answer is not a malformed request.
                    // 502 + Retry-After tells the caller to come back rather
                    // than go debug a payload that was fine.
                    let upstream = is_upstream_rpc_failure(&format!("{e:?}"));
                    let (code, reason, category) = if upstream {
                        (
                            StatusCode::BAD_GATEWAY,
                            "Upstream RPC unavailable for this network; the request was not \
                             rejected, the node could not answer. Retry later.".to_string(),
                            "upstream_rpc_unavailable",
                        )
                    } else {
                        (
                            StatusCode::BAD_REQUEST,
                            format!("Escrow scheme error: {e}"),
                            "escrow_error",
                        )
                    };
                    let mut resp = (
                        code,
                        Json(json!({ "success": false, "errorReason": reason })),
                    )
                        .into_response();
                    if upstream {
                        resp.headers_mut()
                            .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
                    }
                    return Some(AltSchemeOutcome {
                        response: resp,
                        detail: detail(false, escrow_scheme, Some(category)),
                    });
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
                    return Some(AltSchemeOutcome {
                        response: (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "success": false,
                                "errorReason": "Escrow settlement is disabled. Set ENABLE_ESCROW=true to enable."
                            })),
                        )
                            .into_response(),
                        detail: detail(false, Some("refund"), Some("scheme_disabled")),
                    });
                }

                info!("Detected x402r refund extension, routing to escrow settlement");

                match crate::escrow::settle_with_escrow(body_str, &facilitator).await {
                    Ok(escrow_response) => {
                        info!("Escrow settlement complete");
                        let mut d = detail(escrow_response.success, Some("refund"), None);
                        d.network = escrow_response.network.to_string();
                        d.tx = escrow_response.transaction.as_ref().map(|t| t.to_string());
                        d.payer = Some(escrow_response.payer.to_string());
                        return Some(AltSchemeOutcome {
                            response: (StatusCode::OK, Json(escrow_response)).into_response(),
                            detail: d,
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "Escrow settlement failed");
                        return Some(AltSchemeOutcome {
                            response: (
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "success": false,
                                    "errorReason": format!("Escrow error: {}", e)
                                })),
                            )
                                .into_response(),
                            detail: detail(false, Some("refund"), Some("escrow_error")),
                        });
                    }
                }
            }

            // Note: PaymentOperator now uses scheme="escrow" at top level, not extensions
            // The old operator extension pattern is deprecated
        }

        None
    }
    .await;

    if let Some(outcome) = alt_outcome {
        emit_operation(&event_bus, &tx_store, outcome.detail);
        return outcome.response;
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
                            "code": "invalid_request_body",
                            "details": "Check server logs for detailed field-by-field analysis",
                            "hint": "The body must be a JSON object with \
                                     `paymentPayload` and `paymentRequirements`, \
                                     the same shape POST /verify accepts. Both the \
                                     x402 v1 spelling (\"network\": \"base\") and \
                                     the v2 CAIP-2 spelling (\"network\": \
                                     \"eip155:8453\") work. Verify the payload with \
                                     POST /verify first; worked examples: \
                                     https://facilitator.ultravioletadao.xyz/skill.md",
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
            debug!(
                "  - transaction_signature: {}",
                sa_payload.transaction_signature
            );
            debug!(
                "  - settle_secret_key: {}",
                if sa_payload.settle_secret_key.is_some() {
                    "provided"
                } else {
                    "none"
                }
            );
            debug!(
                "  - settlement_rent_destination: {:?}",
                sa_payload.settlement_rent_destination
            );
        }
        #[cfg(feature = "xrpl")]
        crate::types::ExactPaymentPayload::Xrpl(xrpl_payload) => {
            debug!("  - payload type: XRPL (pre-signed Payment)");
            // Use char-safe truncation: signed_tx_blob is hex (pure ASCII) so
            // char indices == byte indices, but .chars().take() is defensive
            // against any future change to the field encoding.
            let preview: String = xrpl_payload.signed_tx_blob.chars().take(100).collect();
            debug!("  - signed_tx_blob: {} (truncated)", preview);
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
            // Live traffic stream — LAST thing after the settle resolved, best-effort and
            // infallible by design (lossy broadcast; see src/events.rs). A subscriber can
            // never slow down or fail a payment.
            event_bus.publish(crate::events::TrafficEvent {
                ts: crate::events::now_ms(),
                kind: "settle",
                // `Display`, NOT `{:?}`: Debug prints the variant name, so multi-word
                // networks came out as `skalebase` / `basesepolia` — names that match
                // nothing in `/supported` and no plate in the observatory. Display is
                // the canonical slug the rest of the facilitator already speaks.
                network: valid_response.network.to_string(),
                ok: valid_response.success,
                payer: Some(valid_response.payer.to_string()),
                tx: valid_response.transaction.as_ref().map(|t| t.to_string()),
                amount: Some(body.payment_requirements.max_amount_required.to_string()),
                asset: Some(body.payment_requirements.asset.to_string()),
                // What was bought, and from whom. The amount alone never said
                // that, which made the stream hard to reason about: two 1-USDC
                // settles look identical until you can see the endpoint.
                resource: Some(body.payment_requirements.resource.to_string()),
                pay_to: Some(body.payment_requirements.pay_to.to_string()),
                description: Some(body.payment_requirements.description.clone()),
                scheme: Some(body.payment_requirements.scheme.to_string()),
                error: None,
            });
            record_transaction(
                &tx_store,
                crate::transaction_store::TransactionRecord {
                    ts: crate::events::now_ms(),
                    kind: "settle".into(),
                    network: valid_response.network.to_string(),
                    ok: valid_response.success,
                    payer: Some(valid_response.payer.to_string()),
                    tx: valid_response.transaction.as_ref().map(|t| t.to_string()),
                    amount: Some(body.payment_requirements.max_amount_required.to_string()),
                    asset: Some(body.payment_requirements.asset.to_string()),
                    resource: Some(body.payment_requirements.resource.to_string()),
                    pay_to: Some(body.payment_requirements.pay_to.to_string()),
                    description: Some(body.payment_requirements.description.clone()),
                    scheme: Some(body.payment_requirements.scheme.to_string()),
                },
            );
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
            // F4: cache the response body so a future client retry with the
            // same Idempotency-Key returns identical bytes without re-running
            // the on-chain settlement. We only cache when settlement reports
            // success — caching failures would lock callers out of legitimate
            // retries against transient errors.
            let cache_response_payload = if valid_response.success {
                serde_json::to_string(&valid_response).ok()
            } else {
                None
            };
            if let (Some(key), Some(hash), Some(json)) = (
                idempotency_key.as_ref(),
                request_hash.as_ref(),
                cache_response_payload.as_ref(),
            ) {
                let expires_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    + IDEMPOTENCY_TTL_SECONDS;
                let record = IdempotencyRecord {
                    idempotency_key: key.clone(),
                    request_hash: hash.clone(),
                    response_json: json.clone(),
                    expires_at,
                };
                let key_for_log = key.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::idempotency_store::store_record(record).await {
                        // Best-effort: cache failure does NOT roll back the
                        // on-chain settlement (the transaction already happened).
                        // Log so retries can be correlated. Fire-and-forget via
                        // tokio::spawn so we don't hold a non-Send future across
                        // the /settle handler's await points.
                        warn!(
                            idempotency_key = %key_for_log,
                            error = %e,
                            "Failed to cache /settle response for idempotency"
                        );
                    }
                });
            }
            // If we already serialized for the cache, reuse it; otherwise
            // serialize once more for the wire. Returning the raw JSON string
            // (instead of Json<SettleResponse>) keeps the wire bytes byte-equal
            // to what we cached, so a cached retry returns the same payload.
            match cache_response_payload {
                Some(json) => (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json,
                )
                    .into_response(),
                None => (StatusCode::OK, Json(valid_response)).into_response(),
            }
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
            publish_failure(
                &event_bus,
                &tx_store,
                "settle",
                &body.payment_requirements,
                &format!("{error:?}"),
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
                // Opaque external error to avoid leaking RPC URLs / revert reasons / API keys
                // that appear in alloy RpcError messages. Full detail logged server-side.
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(%correlation_id, error = %e, "ContractCall error");
                // This is the generic `post_settle` error path -- the >95%
                // EIP-3009 traffic, NOT the escrow branch (escrow errors are
                // `OperatorError`, classified separately at `:2870`/`:3516` and
                // never reach here). Wired to `is_upstream_rpc_failure` on
                // 2026-08-28 (Saul, via team-lead), same predicate the escrow
                // branch already uses: a node that cannot answer (Celo's RPC
                // outage, `txpool is full`, `-32000`/`-32603`/`-32801`) is not
                // a malformed request. Safe to retry as of the fix #1 nonce
                // release (`chain/evm.rs`'s `is_pre_broadcast_rejection`) --
                // before it, a client retrying `txpool is full` burned another
                // nonce and widened the gap instead of curing it.
                //
                // The revert check inside `is_upstream_rpc_failure` runs BEFORE
                // any node-code check, so a genuine contract revert -- bad
                // signature, insufficient balance, an expired/used
                // authorization, ANY custom Solidity error regardless of
                // selector -- still gets 400. See
                // `contract_call_response_tests` below, which exercises this
                // exact arm (not just the classifier) against a revert shaped
                // like `AuthCaptureEscrow.AfterAuthorizationExpiry`
                // (`0x36f2d211`, confirmed against
                // `contracts/out/AuthCaptureEscrow.sol/AuthCaptureEscrow.json`)
                // -- 173 of 226 reverts on 2026-08-19/20 were exactly this, on
                // the escrow path where it was already correctly classified.
                // See docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md.
                if is_upstream_rpc_failure(e) {
                    let mut resp = (
                        StatusCode::BAD_GATEWAY,
                        Json(ErrorResponse {
                            error: format!("upstream_rpc_unavailable (ref: {correlation_id})"),
                        }),
                    )
                        .into_response();
                    resp.headers_mut()
                        .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
                    resp
                } else {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("contract_call_failed (ref: {correlation_id})"),
                        }),
                    )
                        .into_response()
                }
            }
            FacilitatorLocalError::InvalidAddress(ref e) => {
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(%correlation_id, error = %e, "InvalidAddress error");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("invalid_address (ref: {correlation_id})"),
                    }),
                )
                    .into_response()
            }
            FacilitatorLocalError::ClockError(ref e) => {
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(%correlation_id, error = ?e, "ClockError");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("clock_error (ref: {correlation_id})"),
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
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(%correlation_id, error = %e, "Other facilitator error");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("internal_error (ref: {correlation_id})"),
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
            // Same split as the settle path: a node that cannot answer is not a
            // malformed query. This branch was missed when the settle branches
            // were fixed — 9 of the RPC failures observed over 48h came through
            // here and went out as 400, telling callers their request was wrong
            // about an outage they cannot influence.
            if is_upstream_rpc_failure(&format!("{e:?}")) {
                let mut resp = (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "Upstream RPC unavailable for this network; the query was not \
                                  rejected, the node could not answer. Retry later."
                    })),
                )
                    .into_response();
                resp.headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
                return resp;
            }
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
                "rater": "address (recommended) - who is doing the rating. Compared against proof.payer. \
                          A claim, not an authenticated identity: nothing here is signed by the rater yet.",
                // Truth in advertising, resolved at runtime rather than
                // hardcoded: the same binary answers differently depending on
                // whether the operator has switched enforcement on, and a
                // static sentence would be wrong in one of the two states.
                "proofStatus": if crate::erc8004::proof::is_proof_required() {
                    "VERIFIED AND ENFORCED - the facilitator checks the transaction on-chain \
                     (exists, succeeded, right block, right Transfer, payer == rater, payee tied to \
                     the agent, fresh, paymentHash recomputed, not already spent) and REJECTS the \
                     submission when it does not hold."
                } else {
                    "VERIFIED BUT NOT ENFORCED - the facilitator runs every check and reports the \
                     verdict in the response `proof` field, but does NOT reject a failing proof \
                     while ERC8004_REQUIRE_PROOF is off. Do not treat its presence alone as \
                     evidence of payment; read the verdict."
                },
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
            "POST /feedback/revoke": "Revoke previously submitted feedback (ADMIN ONLY: requires Authorization: Bearer <ERC8004_ADMIN_TOKEN>; 404 when no token is configured)",
            "POST /feedback/response": "Append a response to feedback. NOT agent-only, despite what this line used to claim: verified on-chain 2026-08-18, the registry accepts appendResponse from ANY address. This endpoint is unauthenticated and the facilitator signs, so the on-chain `responder` recorded is the FACILITATOR, not the agent.",
            "GET /reputation/:network/:agentId": "Get reputation summary for an agent",
            "GET /identity/:network/:agentId": "Get agent identity from Identity Registry",
            "GET /identity/:network/:agentId/metadata/:key": "Read specific agent metadata",
            "GET /identity/:network/total-supply": "Get total registered agents on a network"
        },
        "supportedNetworks": networks
    }))
}

/// Replay records for the proof gate. Lazily built like the Solana one so a
/// facilitator that never sees a feedback never talks to DynamoDB.
static ERC8004_PROOF_STORE: once_cell::sync::OnceCell<Arc<dyn crate::nonce_store::NonceStore>> =
    once_cell::sync::OnceCell::new();

async fn erc8004_proof_store() -> Arc<dyn crate::nonce_store::NonceStore> {
    if let Some(store) = ERC8004_PROOF_STORE.get() {
        return store.clone();
    }
    let store = crate::nonce_store::create_nonce_store().await;
    let _ = ERC8004_PROOF_STORE.set(store.clone());
    store
}

/// Outcome of trying to spend a proof on one rating.
enum ProofClaim {
    /// Nothing to claim (no proof, or the proof did not verify).
    NotApplicable,
    /// Claimed. Release it if the on-chain write never lands.
    Held(String),
    /// This payment already bought a rating for this agent.
    Replayed,
}

/// Spend the (payment, agent) pair, atomically.
///
/// A payment must not yield fifty ratings. The claim happens BEFORE the
/// on-chain write rather than after, because a check-then-act would let two
/// concurrent requests both pass; the cost of claiming first is that a write
/// which never lands has to give the claim back, which is what
/// `NonceStore::release` is for.
///
/// A store that is unreachable does NOT block the write. The gate is anti-sybil,
/// not custody of funds: losing a replay record costs a duplicate rating, while
/// refusing every rating because DynamoDB blinked costs real reputation. The
/// failure is logged loudly instead.
async fn claim_feedback_proof(
    network: &crate::network::Network,
    proof: &crate::erc8004::ProofOfPayment,
    agent_id: &str,
) -> ProofClaim {
    let key = crate::erc8004::proof::proof_replay_key(network, &proof.transaction_hash, agent_id);
    let store = erc8004_proof_store().await;
    match store
        .check_and_mark_used(&key, crate::erc8004::proof::replay_ttl_secs())
        .await
    {
        Ok(()) => ProofClaim::Held(key),
        Err(crate::nonce_store::NonceStoreError::NonceAlreadyUsed(_)) => {
            warn!(
                network = %network,
                agent_id = %agent_id,
                "proof replay: this payment already bought a rating for this agent"
            );
            ProofClaim::Replayed
        }
        Err(e) => {
            error!(
                network = %network,
                agent_id = %agent_id,
                error = %crate::redact::scrub_urls(&e.to_string()),
                "proof replay store unavailable; proceeding WITHOUT replay protection"
            );
            ProofClaim::NotApplicable
        }
    }
}

/// Give a claim back after a write that never landed.
async fn release_feedback_proof(claim: &ProofClaim) {
    if let ProofClaim::Held(key) = claim {
        let store = erc8004_proof_store().await;
        if let Err(e) = store.release(key).await {
            // Not fatal: the key stays claimed, which costs the caller a retry
            // and never costs a duplicate rating.
            warn!(
                error = %crate::redact::scrub_urls(&e.to_string()),
                "could not release the proof claim after a failed feedback write"
            );
        }
    }
}

/// One line per submission carrying the verdict, in phase 1 and phase 2 alike.
///
/// This IS the measurement the two-phase rollout is for: with
/// `ERC8004_REQUIRE_PROOF` off, these lines are how we find out how much real
/// traffic a hard gate would break before it breaks it.
fn log_proof_verdict(
    network: &crate::network::Network,
    agent_id: &str,
    report: &crate::erc8004::proof::ProofReport,
) {
    match report.rejection {
        None => info!(
            network = %network,
            agent_id = %agent_id,
            anchor = report.anchor.as_str(),
            "[OK] feedback proof verified"
        ),
        Some(reason) => warn!(
            network = %network,
            agent_id = %agent_id,
            reason = reason.as_str(),
            anchor = report.anchor.as_str(),
            enforced = report.enforced,
            "[WARN] feedback proof did not verify"
        ),
    }
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
                    proof: None,
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
                proof: None,
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
    let agent_id_str =
        parse_agent_id_value(&feedback.agent_id).unwrap_or_else(|| feedback.agent_id.to_string());

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
                            proof: None,
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some(format!(
                                "No Solana ERC-8004 programs for network {}",
                                network
                            )),
                            network,
                        }),
                    )
                        .into_response();
                }
            };

            let asset_pubkey = match solana_erc8004::parse_agent_id(&agent_id_str) {
                Ok(pk) => pk,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            proof: None,
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some(format!("{}", e)),
                            network,
                        }),
                    )
                        .into_response();
                }
            };

            // give_feedback verifies the agent NFT belongs to the registry collection
            let collection = match solana_erc8004::read_collection_pubkey(
                p.rpc_client(),
                &programs.agent_registry,
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    error!(network = %network, error = %e, "Failed to read collection pubkey");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(FeedbackResponse {
                            proof: None,
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some(format!("Failed to read registry config: {}", e)),
                            network,
                        }),
                    )
                        .into_response();
                }
            };

            let fee_payer = p.keypair().pubkey();
            let feedback_hash_bytes: Option<[u8; 32]> = feedback.feedback_hash.map(|h| h.0);

            // Without a score the ATOM Engine records the feedback but scores nothing,
            // so reputation would stay at zero however much feedback arrives.
            let score = feedback.score;
            if let Some(s) = score {
                if s > 100 {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            proof: None,
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some("score must be between 0 and 100".to_string()),
                            network,
                        }),
                    )
                        .into_response();
                }
            } else {
                warn!(
                    network = %network,
                    "Feedback submitted without a score: it will not affect ATOM reputation"
                );
            }

            // The document half of the proof gate runs here too -- keccak does
            // not care which chain anchors it -- but the payment half does not:
            // checking an SVM payment means reading an SVM transaction, which
            // the gate does not do. Recorded as an explicit gap
            // (`proof_unverifiable_chain`) that never blocks the write, rather
            // than as a check that quietly did not run.
            let proof_report = crate::erc8004::proof::evaluate_svm_feedback_proof(feedback).await;
            log_proof_verdict(&network, &agent_id_str, &proof_report);

            // DEPRECATED authorship path. `client` below is the facilitator's
            // own keypair, so the chain records US as the author of somebody
            // else's opinion -- the defect this whole workstream exists to
            // close. `/feedback/solana/prepare` + `/feedback/solana/submit`
            // does the same write with the rater signing as `client`.
            //
            // Left ON by default so integrations do not break the day this
            // ships, behind a switch so an operator can close it, and loud in
            // the logs so the deprecation is visible instead of theoretical.
            if !solana_erc8004::is_facilitator_authorship_allowed() {
                warn!(
                    network = %network,
                    agent_id = %agent_id_str,
                    "refusing facilitator-authored Solana feedback: \
                     ERC8004_ALLOW_FACILITATOR_AUTHORSHIP=false"
                );
                return (
                    StatusCode::BAD_REQUEST,
                    Json(FeedbackResponse {
                        proof: Some(proof_report),
                        success: false,
                        transaction: None,
                        feedback_index: None,
                        error: Some(
                            "this facilitator no longer signs feedback as the author; \
                             use POST /feedback/solana/prepare and /feedback/solana/submit \
                             so the rater signs as `client`"
                                .to_string(),
                        ),
                        network,
                    }),
                )
                    .into_response();
            }
            warn!(
                network = %network,
                agent_id = %agent_id_str,
                "[WARN] DEPRECATED: writing Solana feedback authored by the FACILITATOR, \
                 not by the rater. Use /feedback/solana/prepare + /feedback/solana/submit"
            );

            let ix = solana_erc8004::build_give_feedback_ix(
                &programs,
                &collection,
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

            match solana_erc8004::send_erc8004_transaction(p.rpc_client(), p.keypair(), vec![ix])
                .await
            {
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
                            proof: Some(proof_report),
                            success: true,
                            transaction: Some(crate::types::TransactionHash::Solana(sig.into())),
                            feedback_index: None,
                            error: None,
                            network,
                        }),
                    )
                        .into_response()
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Solana feedback transaction failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(FeedbackResponse {
                            proof: None,
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
        Some(NetworkProvider::Evm(provider)) => {
            // ── EVM feedback via IReputationRegistry.giveFeedback ──
            let contracts = match get_contracts(&network) {
                Some(c) => c,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            proof: None,
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some(format!("No ERC-8004 contracts for network {}", network)),
                            network,
                        }),
                    )
                        .into_response();
                }
            };

            let agent_id_u64: u64 = match agent_id_str.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            proof: None,
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some(format!(
                                "Invalid EVM agent ID (expected numeric): {}",
                                agent_id_str
                            )),
                            network,
                        }),
                    )
                        .into_response();
                }
            };

            // ── Proof-of-payment gate (anti-sybil) ──
            //
            // The registry lets any address rate any agent, so without this the
            // only thing rationing reputation is the fact that we are the ones
            // signing. Two-phase by design: with ERC8004_REQUIRE_PROOF off the
            // verdict is measured and logged, not enforced.
            let proof_report = crate::erc8004::proof::evaluate_feedback_proof(
                provider.inner(),
                contracts.identity_registry,
                network,
                agent_id_u64,
                feedback,
            )
            .await;
            log_proof_verdict(&network, &agent_id_str, &proof_report);

            // DEPRECATED authorship path, same shape as the SVM one above: the
            // registry records `msg.sender`, and on this route that is US. Where
            // a FeedbackDelegate is deployed there is now a route that writes
            // the same rating with the RATER as author, so say so on every call
            // instead of leaving the deprecation theoretical.
            //
            // Warn-only on purpose. Closing this route is a separate decision
            // with its own switch (see ERC8004_ALLOW_FACILITATOR_AUTHORSHIP on
            // the SVM side); turning a log line into a rejection here would
            // break every caller the day mainnet delegates landed.
            if crate::erc8004::relay::feedback_delegate(&network).is_some() {
                warn!(
                    network = %network,
                    agent_id = %agent_id_str,
                    "[WARN] DEPRECATED: writing EVM feedback authored by the FACILITATOR, \
                     not by the rater. Use /feedback/evm/prepare + /feedback/evm/submit"
                );
            }

            if proof_report.should_reject() {
                let reason = proof_report
                    .rejection
                    .map(|r| r.as_str())
                    .unwrap_or("proof_rejected");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(FeedbackResponse {
                        // A BOUNDED reason, never the raw error: those carry
                        // addresses and RPC URLs with keys in them.
                        proof: Some(proof_report),
                        success: false,
                        transaction: None,
                        feedback_index: None,
                        error: Some(format!("proof of payment rejected: {}", reason)),
                        network,
                    }),
                )
                    .into_response();
            }

            // Spend the proof only once it has actually verified: claiming on a
            // proof we just refused would burn a key on the strength of garbage.
            let claim = match (proof_report.is_verified(), feedback.proof.as_ref()) {
                (true, Some(p)) => claim_feedback_proof(&network, p, &agent_id_str).await,
                _ => ProofClaim::NotApplicable,
            };
            if matches!(claim, ProofClaim::Replayed) && crate::erc8004::proof::is_proof_required() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(FeedbackResponse {
                        proof: Some(proof_report),
                        success: false,
                        transaction: None,
                        feedback_index: None,
                        error: Some(format!(
                            "proof of payment rejected: {}",
                            crate::erc8004::proof::ProofRejection::Replayed.as_str()
                        )),
                        network,
                    }),
                )
                    .into_response();
            }

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

            // KNOWN GAP (2026-08-28,
            // docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md):
            // both `.send().await` calls below skip the estimate-first guard that
            // `EvmProvider::settle()` uses (`chain/evm.rs`, next to
            // `PendingNonceManager`'s doc comment). Sending straight to
            // `.send()` lets alloy's `JoinFill` fill gas AND nonce CONCURRENTLY
            // (`try_join!`): `NonceFiller::prepare` commits the nonce reservation
            // before gas estimation can even fail, so a call that reverts on
            // estimation still burns the nonce it never broadcast. `/settle`
            // dodges this by calling `estimate_gas` before reserving a nonce at
            // all; this handler — and 4 others sharing the same
            // `PendingNonceManager` (`post_revoke_feedback`,
            // `post_append_response`, `run_evm_registration`,
            // `transfer_agent_nft`) — do not.
            //
            // Measured cost on Monad (2026-08-24, no global mempool to absorb
            // the gap): a reverting `/feedback` at 03:58:40 burned a nonce;
            // nonces 379/380/381 sat unmined for 151-283s until nonce 378
            // finally landed at 04:02:57 and unstuck all three at once. The
            // distribution was bimodal (0-1s or 151-283s, nothing between) —
            // the gap does not fail transactions, it FREEZES them until
            // something fills the hole.
            //
            // Deliberately not fixed here: `run_evm_registration` carries a
            // `pending -> mint_confirmed -> done/failed` job state machine that
            // a rushed guard could break, and none of the 5 call sites have a
            // nonce-reservation regression test today. Extending the guard is
            // a scoped follow-up, not a drive-by edit.
            //
            // Legacy chains (SKALE) need explicit gasPrice to avoid EIP-1559 rejection
            let send_result = if !provider.is_eip1559() {
                let gp = provider
                    .inner()
                    .get_gas_price()
                    .await
                    .map_err(|e| format!("{e:?}"));
                match gp {
                    Ok(gas_price) => call.gas_price(gas_price).send().await,
                    Err(e) => {
                        error!(error = %e, "Failed to get gas price");
                        // Nothing was written, so the proof has not been spent.
                        release_feedback_proof(&claim).await;
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(FeedbackResponse {
                                proof: Some(proof_report),
                                success: false,
                                transaction: None,
                                feedback_index: None,
                                error: Some(format!("Failed to get gas price: {}", e)),
                                network,
                            }),
                        )
                            .into_response();
                    }
                }
            } else {
                call.send().await
            };

            match send_result {
                Ok(pending_tx) => match pending_tx.get_receipt().await {
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
                                proof: Some(proof_report),
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
                        error!(network = %network, error = %e, "Failed to get transaction receipt");
                        // The receipt never arrived, so we do NOT know whether
                        // the write landed. Keeping the claim is the safe
                        // direction: a lost rating can be retried by a human, a
                        // duplicated one cannot be taken back.
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(FeedbackResponse {
                                proof: Some(proof_report),
                                success: false,
                                transaction: None,
                                feedback_index: None,
                                error: Some(format!("Transaction failed: {}", e)),
                                network,
                            }),
                        )
                            .into_response()
                    }
                },
                Err(e) => {
                    error!(network = %network, error = %e, "Failed to submit feedback transaction");
                    // The submission itself was refused, so nothing was written.
                    release_feedback_proof(&claim).await;
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(FeedbackResponse {
                            proof: Some(proof_report),
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
        _ => {
            error!(network = %network, "No provider available for network");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FeedbackResponse {
                    proof: None,
                    success: false,
                    transaction: None,
                    feedback_index: None,
                    error: Some(format!("No provider available for network {}", network)),
                    network,
                }),
            )
                .into_response()
        }
    }
}

/// Everything both halves of the EIP-7702 relayed-feedback flow need, resolved
/// once so `prepare` and `submit` cannot drift apart.
///
/// If they derived the calldata differently, the rater's signature would cover
/// one call and we would relay another -- and the digest check would be
/// comparing two things we made up.
struct RelayContext {
    delegate: alloy::primitives::Address,
    /// Which delegate version is actually deployed on this chain, read from
    /// the chain on this request. Decides which digest the rater signs.
    version: crate::erc8004::relay::DelegateVersion,
    registry: alloy::primitives::Address,
    rater: alloy::primitives::Address,
    agent_id: u64,
    chain_id: u64,
    data: alloy::primitives::Bytes,
}

async fn relay_context(
    provider: &crate::chain::evm::EvmProvider,
    network: crate::network::Network,
    feedback: &crate::erc8004::FeedbackParams,
) -> Result<RelayContext, (StatusCode, String)> {
    use crate::chain::evm::MetaEvmProvider as _;

    let contracts = get_contracts(&network).ok_or((
        StatusCode::BAD_REQUEST,
        format!("No ERC-8004 contracts for network {}", network),
    ))?;

    let delegate = crate::erc8004::relay::feedback_delegate(&network).ok_or((
        StatusCode::BAD_REQUEST,
        format!(
            "relayed feedback is not available on {}: no FeedbackDelegate is deployed there yet",
            network
        ),
    ))?;

    let rater = match feedback.rater.as_ref() {
        Some(mixed) => alloy::primitives::Address::try_from(mixed.clone()).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "rater must be an EVM address on this network".to_string(),
            )
        })?,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                "rater is required: it is the account whose EOA is delegated and \
                 whose key authorises the rating, which is the entire point of \
                 this endpoint"
                    .to_string(),
            ))
        }
    };

    let agent_id_str =
        parse_agent_id_value(&feedback.agent_id).unwrap_or_else(|| feedback.agent_id.to_string());
    let agent_id: u64 = agent_id_str.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid EVM agent ID (expected numeric): {}", agent_id_str),
        )
    })?;

    // An address in a table is a claim; eth_getCode is evidence. A delegate
    // address with no contract behind it would produce a transaction that looks
    // like it worked and rated nobody.
    // ...and the VERSION comes from the same read, because an address does not
    // carry one: the deploys use CREATE, so the same address is a different
    // contract on a different chain. Detecting per request is also what lets
    // Execution Market roll v4 chain by chain without us coordinating a deploy.
    let version = crate::erc8004::relay::assert_delegate_usable(
        provider.inner(),
        delegate,
        contracts.reputation_registry,
    )
    .await
    .map_err(|e| {
        error!(network = %network, reason = e.as_str(), "FeedbackDelegate is not usable");
        (StatusCode::SERVICE_UNAVAILABLE, e.as_str().to_string())
    })?;

    let data = crate::erc8004::relay::give_feedback_calldata(
        agent_id,
        feedback.value,
        feedback.value_decimals,
        &feedback.tag1,
        &feedback.tag2,
        &feedback.endpoint,
        &feedback.feedback_uri,
        feedback.feedback_hash.unwrap_or_default(),
    );

    Ok(RelayContext {
        delegate,
        version,
        registry: contracts.reputation_registry,
        rater,
        agent_id,
        chain_id: provider.chain().chain_id,
        data,
    })
}

/// `POST /feedback/evm/prepare`: everything the rater must sign so the chain
/// records THEM as the author.
///
/// The ERC-8004 Reputation Registry records `msg.sender` as the author and the
/// deployed implementation has no delegation path at all -- no
/// `giveFeedbackWithSignature`, no ERC-2771 forwarder. So a rating we relay
/// normally is a rating attributed to US: 87,2% of the feedback on Base, and the
/// same address can revoke any of it.
///
/// EIP-7702 fixes it without touching the registry: the rater delegates their
/// own EOA to the FeedbackDelegate, and we send the transaction TO THE RATER'S
/// ADDRESS, so the registry sees the rater as `msg.sender` while we pay.
#[instrument(skip_all)]
pub async fn post_prepare_relay_feedback<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    use crate::chain::evm::MetaEvmProvider as _;

    let request: FeedbackRequest =
        match serde_json::from_slice(&raw_body) {
            Ok(r) => r,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Invalid request format: {}", e)})),
            )
                .into_response(),
        };
    let network = request.network;
    let feedback = &request.feedback;

    let provider_map = facilitator.provider_map();
    let Some(NetworkProvider::Evm(provider)) = provider_map.by_network(&network) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("{} is not an EVM network served by this facilitator", network)
            })),
        )
            .into_response();
    };

    let ctx = match relay_context(provider, network, feedback).await {
        Ok(c) => c,
        Err((code, msg)) => {
            return (code, Json(json!({"success": false, "error": msg}))).into_response()
        }
    };

    let state = match crate::erc8004::relay::delegation_state(
        provider.inner(),
        ctx.rater,
        ctx.delegate,
        ctx.registry,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"success": false, "error": e.as_str()})),
            )
                .into_response()
        }
    };
    if state == crate::erc8004::relay::DelegationState::Foreign {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": crate::erc8004::relay::RelayError::ForeignDelegation.as_str()
            })),
        )
            .into_response();
    }
    // A rater still pointed at a SUPERSEDED version of our own delegate is not
    // delegated for our purposes: they need a fresh authorisation, exactly like
    // a plain EOA. Reporting them as delegated would send a type-4 transaction
    // that runs the old contract; reporting them as Foreign would lock out
    // everyone who ever rated, the day we move a version.
    let delegated = state == crate::erc8004::relay::DelegationState::Delegated;

    let account_nonce = if delegated {
        None
    } else {
        match provider.inner().get_transaction_count(ctx.rater).await {
            Ok(n) => Some(n),
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"success": false, "error": "relay_rpc_unavailable"})),
                )
                    .into_response()
            }
        }
    };

    // Short deadline: `relayFeedback` is permissionless by design, so a signed
    // authorisation is live in the wild until it expires. Minutes, not forever.
    let deadline = crate::erc8004::proof::unix_now_secs()
        .saturating_add(crate::erc8004::relay::relay_deadline_secs());
    let nonce: alloy::primitives::FixedBytes<32> = {
        use rand::RngCore as _;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        alloy::primitives::FixedBytes::from(bytes)
    };
    // Both versions are served in parallel, chosen by what is deployed on THIS
    // chain right now. Execution Market can therefore roll v4 network by network
    // without a deploy of ours in between, and a client mid-migration never ends
    // up unable to sign.
    let (digest, signing_payload, typed_data) = match ctx.version {
        crate::erc8004::relay::DelegateVersion::V4 => {
            let p = crate::erc8004::relay_v4::give_feedback_params(
                ctx.registry,
                ctx.agent_id,
                feedback.value,
                feedback.value_decimals,
                &feedback.tag1,
                &feedback.tag2,
                &feedback.endpoint,
                &feedback.feedback_uri,
                feedback.feedback_hash.unwrap_or_default(),
                deadline,
                nonce,
            );
            (
                crate::erc8004::relay_v4::give_feedback_digest(ctx.chain_id, ctx.rater, &p),
                // v4 needs no `signingPayload`: `signTypedData` has no envelope
                // to apply twice, which is the whole class of bug it removes.
                None,
                Some(crate::erc8004::relay_v4::give_feedback_typed_data(
                    ctx.chain_id,
                    ctx.rater,
                    &p,
                )),
            )
        }
        crate::erc8004::relay::DelegateVersion::V3 => {
            let digest = crate::erc8004::relay::relay_digest(
                ctx.chain_id,
                ctx.rater,
                ctx.registry,
                &ctx.data,
                deadline,
                nonce,
            );
            // What a wallet signs on v3. `digest` already carries the EIP-191
            // envelope, so a wallet handed that value wraps it a second time and
            // recovers a stranger.
            let payload = crate::erc8004::relay::relay_signing_payload(
                ctx.chain_id,
                ctx.rater,
                ctx.registry,
                &ctx.data,
                deadline,
                nonce,
            );
            (digest, Some(payload), None)
        }
    };

    (
        StatusCode::OK,
        Json(crate::erc8004::PrepareRelayFeedbackResponse {
            success: true,
            delegate: Some(ctx.delegate),
            data: Some(ctx.data),
            digest: Some(digest),
            signing_payload,
            typed_data,
            deadline: Some(deadline),
            nonce: Some(nonce),
            delegated,
            account_nonce,
            chain_id: ctx.chain_id,
            error: None,
            network,
        }),
    )
        .into_response()
}

/// `POST /feedback/evm/submit`: relay a rater-authorised rating as a type-4
/// transaction, paying the gas without becoming the author.
///
/// Same discipline as the Solana path: the facilitator does NOT relay what it is
/// handed. It rebuilds the registry calldata from the declared parameters and
/// requires the rater's signature to cover exactly that. The delegate is a
/// second line of defence -- it accepts two selectors and can never move funds --
/// but a facilitator that relayed arbitrary calldata would be leaning on
/// somebody else's audit instead of doing its own check.
#[instrument(skip_all)]
pub async fn post_submit_relay_feedback<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    use crate::chain::evm::MetaEvmProvider as _;
    use crate::erc8004::relay::{self, DelegationState, RelayError};

    let request: crate::erc8004::SubmitRelayFeedbackRequest =
        match serde_json::from_slice(&raw_body) {
            Ok(r) => r,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Invalid request format: {}", e)})),
            )
                .into_response(),
        };
    let network = request.network;
    let feedback = &request.feedback;

    let provider_map = facilitator.provider_map();
    let Some(NetworkProvider::Evm(provider)) = provider_map.by_network(&network) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("{} is not an EVM network served by this facilitator", network)
            })),
        )
            .into_response();
    };

    let ctx = match relay_context(provider, network, feedback).await {
        Ok(c) => c,
        Err((code, msg)) => {
            return (code, Json(json!({"success": false, "error": msg}))).into_response()
        }
    };
    let agent_id_str = ctx.agent_id.to_string();

    let refuse = |e: RelayError, report: Option<crate::erc8004::proof::ProofReport>| {
        warn!(
            network = %network,
            reason = e.as_str(),
            "refusing to relay an ERC-8004 feedback"
        );
        (
            StatusCode::BAD_REQUEST,
            Json(FeedbackResponse {
                proof: report,
                success: false,
                transaction: None,
                feedback_index: None,
                error: Some(e.as_str().to_string()),
                network,
            }),
        )
            .into_response()
    };

    if request.deadline <= crate::erc8004::proof::unix_now_secs() {
        return refuse(RelayError::Expired, None);
    }

    // The signature must cover what WE rebuilt, not what we were handed -- in
    // either version. v4 rebuilds the typed struct from the declared parameters
    // and v3 rebuilds the calldata; both then require the rater's signature to
    // cover exactly that.
    let v4_params = match ctx.version {
        relay::DelegateVersion::V4 => {
            let p = crate::erc8004::relay_v4::give_feedback_params(
                ctx.registry,
                ctx.agent_id,
                feedback.value,
                feedback.value_decimals,
                &feedback.tag1,
                &feedback.tag2,
                &feedback.endpoint,
                &feedback.feedback_uri,
                feedback.feedback_hash.unwrap_or_default(),
                request.deadline,
                request.nonce,
            );
            if let Err(e) = crate::erc8004::relay_v4::signature_authorises(
                ctx.chain_id,
                ctx.rater,
                &p,
                &request.signature,
            ) {
                return refuse(e, None);
            }
            Some(p)
        }
        relay::DelegateVersion::V3 => {
            let digest = relay::relay_digest(
                ctx.chain_id,
                ctx.rater,
                ctx.registry,
                &ctx.data,
                request.deadline,
                request.nonce,
            );
            if !relay::signature_authorises(digest, &request.signature, ctx.rater) {
                return refuse(RelayError::BadSignature, None);
            }
            None
        }
    };

    let state = match relay::delegation_state(
        provider.inner(),
        ctx.rater,
        ctx.delegate,
        ctx.registry,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return refuse(e, None),
    };
    if state == DelegationState::Foreign {
        return refuse(RelayError::ForeignDelegation, None);
    }

    // Only needed when the account is not delegated yet; installing it again
    // would just burn an account nonce.
    let authorization_list = if state == DelegationState::Delegated {
        // The delegate stores spent nonces in the RATER's own storage, so this
        // reads the rater's address. It is only meaningful once the account
        // carries the code.
        match relay::nonce_already_used(provider.inner(), ctx.rater, request.nonce).await {
            Ok(true) => return refuse(RelayError::NonceAlreadyUsed, None),
            Ok(false) => {}
            Err(e) => return refuse(e, None),
        }
        None
    } else {
        let Some(params) = request.authorization.as_ref() else {
            return refuse(RelayError::MissingAuthorization, None);
        };
        let signed = relay::signed_authorization(
            alloy::primitives::U256::from(params.chain_id),
            params.address,
            params.nonce,
            params.y_parity,
            params.r,
            params.s,
        );
        if let Err(e) = relay::accept_authorization(&signed, ctx.rater, ctx.delegate, ctx.chain_id)
        {
            return refuse(e, None);
        }
        Some(vec![signed])
    };

    // The proof gate applies to a relayed rating exactly as it does to a direct
    // one: who authored it and whether a payment backs it are separate questions.
    let proof_report = crate::erc8004::proof::evaluate_feedback_proof(
        provider.inner(),
        match get_contracts(&network) {
            Some(c) => c.identity_registry,
            None => alloy::primitives::Address::ZERO,
        },
        network,
        ctx.agent_id,
        feedback,
    )
    .await;
    log_proof_verdict(&network, &agent_id_str, &proof_report);
    if proof_report.should_reject() {
        let reason = proof_report
            .rejection
            .map(|r| r.as_str())
            .unwrap_or("proof_rejected");
        return (
            StatusCode::BAD_REQUEST,
            Json(FeedbackResponse {
                proof: Some(proof_report),
                success: false,
                transaction: None,
                feedback_index: None,
                error: Some(format!("proof of payment rejected: {}", reason)),
                network,
            }),
        )
            .into_response();
    }

    let claim = match (proof_report.is_verified(), feedback.proof.as_ref()) {
        (true, Some(p)) => claim_feedback_proof(&network, p, &agent_id_str).await,
        _ => ProofClaim::NotApplicable,
    };
    if matches!(claim, ProofClaim::Replayed) && crate::erc8004::proof::is_proof_required() {
        return (
            StatusCode::BAD_REQUEST,
            Json(FeedbackResponse {
                proof: Some(proof_report),
                success: false,
                transaction: None,
                feedback_index: None,
                error: Some(format!(
                    "proof of payment rejected: {}",
                    crate::erc8004::proof::ProofRejection::Replayed.as_str()
                )),
                network,
            }),
        )
            .into_response();
    }

    // v4's entry point takes the typed struct itself, so the contract builds the
    // registry calldata from the very thing that was signed and displayed. v3
    // takes the pre-encoded calldata plus its window.
    let calldata = match v4_params {
        Some(ref p) => {
            crate::erc8004::relay_v4::relay_give_feedback_calldata(p, &request.signature)
        }
        None => relay::relay_feedback_calldata(
            &ctx.data,
            request.deadline,
            request.nonce,
            &request.signature,
        ),
    };

    // Sent TO THE RATER'S ADDRESS: that is what makes the registry observe the
    // rater as msg.sender while our EOA only pays.
    let meta = crate::chain::evm::MetaTransaction {
        to: ctx.rater,
        calldata,
        confirmations: 1,
        authorization_list,
    };

    match provider
        .send_transaction_from(provider.pinned_signer(), meta)
        .await
    {
        Ok(receipt) => {
            let tx_hash = receipt.transaction_hash;
            info!(
                network = %network,
                tx = %tx_hash,
                agent_id = %agent_id_str,
                rater = %ctx.rater,
                "[OK] ERC-8004 feedback relayed with the RATER as author"
            );
            (
                StatusCode::OK,
                Json(FeedbackResponse {
                    proof: Some(proof_report),
                    success: true,
                    transaction: Some(crate::types::TransactionHash::Evm(tx_hash.0)),
                    feedback_index: None,
                    error: None,
                    network,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(network = %network, error = %e, "relayed feedback transaction failed");
            // Nothing landed, so the proof has not been spent.
            release_feedback_proof(&claim).await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FeedbackResponse {
                    proof: Some(proof_report),
                    success: false,
                    transaction: None,
                    feedback_index: None,
                    error: Some("relayed feedback transaction failed".to_string()),
                    network,
                }),
            )
                .into_response()
        }
    }
}

/// Shared setup for both halves of the partially-signed Solana feedback flow:
/// resolve the programs, the agent asset, the collection and the rater.
///
/// Returned as a tuple rather than inlined twice so `prepare` and `submit`
/// cannot drift: if they built the instruction from different inputs, the
/// byte-for-byte comparison in `submit` would be comparing two things we made
/// up, and the guarantee would be theatre.
struct SvmFeedbackContext {
    programs: solana_erc8004::SolanaErc8004Programs,
    collection: solana_sdk::pubkey::Pubkey,
    asset: solana_sdk::pubkey::Pubkey,
    rater: solana_sdk::pubkey::Pubkey,
}

async fn svm_feedback_context(
    provider: &crate::chain::solana::SolanaProvider,
    network: crate::network::Network,
    feedback: &crate::erc8004::FeedbackParams,
) -> Result<SvmFeedbackContext, (StatusCode, String)> {
    let programs = solana_erc8004::get_program_ids(&network).ok_or((
        StatusCode::BAD_REQUEST,
        format!("No Solana ERC-8004 programs for network {}", network),
    ))?;

    let agent_id_str =
        parse_agent_id_value(&feedback.agent_id).unwrap_or_else(|| feedback.agent_id.to_string());
    let asset = solana_erc8004::parse_agent_id(&agent_id_str)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{}", e)))?;

    let rater = match feedback.rater.as_ref() {
        Some(MixedAddress::Solana(pk)) => *pk,
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "rater must be a base58 Solana pubkey on this network".to_string(),
            ))
        }
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                "rater is required: it is the account that signs as `client`, and \
                 the whole point of this endpoint is that the chain records the \
                 rater as the author instead of the facilitator"
                    .to_string(),
            ))
        }
    };
    if let Some(score) = feedback.score {
        if score > 100 {
            return Err((
                StatusCode::BAD_REQUEST,
                "score must be between 0 and 100".to_string(),
            ));
        }
    }

    let collection =
        solana_erc8004::read_collection_pubkey(provider.rpc_client(), &programs.agent_registry)
            .await
            .map_err(|e| {
                error!(network = %network, error = %e, "Failed to read collection pubkey");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Failed to read registry config".to_string(),
                )
            })?;

    Ok(SvmFeedbackContext {
        programs,
        collection,
        asset,
        rater,
    })
}

/// `POST /feedback/solana/prepare`: build the feedback transaction for the RATER
/// to sign.
///
/// Account 0 of the program's `give_feedback` instruction is declared
/// `[signer, writable] client (feedback author / fee payer)`, and the facilitator
/// had been putting its own keypair there -- which is why the chain records US as
/// the author of the overwhelming majority of feedback. Solana supports several
/// signers per transaction natively, so the fix needs no delegation and no
/// program change: the rater signs as `client`, we stay the fee payer.
///
/// It does change the contract of the endpoint, though. This is no longer
/// "send me the data and I will write it"; it is "sign this and I will pay for
/// it", which is a different (and honest) relationship.
#[instrument(skip_all)]
pub async fn post_prepare_solana_feedback<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    let request: FeedbackRequest =
        match serde_json::from_slice(&raw_body) {
            Ok(r) => r,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Invalid request format: {}", e)})),
            )
                .into_response(),
        };
    let network = request.network;
    let feedback = &request.feedback;

    let provider_map = facilitator.provider_map();
    let Some(NetworkProvider::Solana(provider)) = provider_map.by_network(&network) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("{} is not a Solana network served by this facilitator", network)
            })),
        )
            .into_response();
    };

    let ctx = match svm_feedback_context(provider, network, feedback).await {
        Ok(c) => c,
        Err((code, msg)) => {
            return (code, Json(json!({"success": false, "error": msg}))).into_response()
        }
    };

    let (blockhash, last_valid_block_height) = match provider
        .rpc_client()
        .get_latest_blockhash_with_commitment(
            solana_commitment_config::CommitmentConfig::finalized(),
        )
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            error!(network = %network, error = %e, "Failed to get blockhash");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"success": false, "error": "could not reach the network"})),
            )
                .into_response();
        }
    };

    let fee_payer = provider.keypair().pubkey();
    let tx = solana_erc8004::build_feedback_transaction(
        &ctx.programs,
        &ctx.collection,
        &ctx.asset,
        &ctx.rater,
        &fee_payer,
        feedback.value,
        feedback.value_decimals,
        feedback.score,
        &feedback.tag1,
        &feedback.tag2,
        &feedback.endpoint,
        &feedback.feedback_uri,
        feedback.feedback_hash.map(|h| h.0),
        blockhash,
    );

    match solana_erc8004::encode_transaction(&tx) {
        Ok(encoded) => (
            StatusCode::OK,
            Json(crate::erc8004::PrepareFeedbackResponse {
                success: true,
                transaction: Some(encoded),
                rater: Some(MixedAddress::Solana(ctx.rater)),
                fee_payer: Some(MixedAddress::Solana(fee_payer)),
                blockhash: Some(blockhash.to_string()),
                last_valid_block_height: Some(last_valid_block_height),
                error: None,
                network,
            }),
        )
            .into_response(),
        Err(e) => {
            error!(network = %network, error = %e, "Failed to encode feedback transaction");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": "could not encode the transaction"})),
            )
                .into_response()
        }
    }
}

/// `POST /feedback/solana/submit`: co-sign and send a rater-signed feedback
/// transaction.
///
/// The security boundary of the flow lives here. We do NOT sign what we are
/// given: we rebuild the message from the declared parameters and the blockhash
/// carried by the submission, and refuse anything that is not byte-for-byte what
/// we would have offered. Signing an arbitrary blob would hand the fee-payer
/// keypair to whoever asks -- one `system_program::transfer` and the wallet is
/// empty, with our signature on it.
#[instrument(skip_all)]
pub async fn post_submit_solana_feedback<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    let request: crate::erc8004::SubmitFeedbackRequest =
        match serde_json::from_slice(&raw_body) {
            Ok(r) => r,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Invalid request format: {}", e)})),
            )
                .into_response(),
        };
    let network = request.network;
    let feedback = &request.feedback;

    let provider_map = facilitator.provider_map();
    let Some(NetworkProvider::Solana(provider)) = provider_map.by_network(&network) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("{} is not a Solana network served by this facilitator", network)
            })),
        )
            .into_response();
    };

    let submitted = match solana_erc8004::decode_transaction(&request.transaction) {
        Ok(tx) => tx,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("{}", e)})),
            )
                .into_response()
        }
    };

    let ctx = match svm_feedback_context(provider, network, feedback).await {
        Ok(c) => c,
        Err((code, msg)) => {
            return (code, Json(json!({"success": false, "error": msg}))).into_response()
        }
    };

    // The document half of the proof gate still applies here.
    let proof_report = crate::erc8004::proof::evaluate_svm_feedback_proof(feedback).await;
    let agent_id_str =
        parse_agent_id_value(&feedback.agent_id).unwrap_or_else(|| feedback.agent_id.to_string());
    log_proof_verdict(&network, &agent_id_str, &proof_report);

    // Rebuild from the DECLARED parameters, using the blockhash the caller
    // brought back. Everything else is ours.
    let fee_payer = provider.keypair().pubkey();
    let expected = solana_erc8004::build_feedback_transaction(
        &ctx.programs,
        &ctx.collection,
        &ctx.asset,
        &ctx.rater,
        &fee_payer,
        feedback.value,
        feedback.value_decimals,
        feedback.score,
        &feedback.tag1,
        &feedback.tag2,
        &feedback.endpoint,
        &feedback.feedback_uri,
        feedback.feedback_hash.map(|h| h.0),
        submitted.message.recent_blockhash,
    );

    if let Err(e) =
        solana_erc8004::accept_rater_signed_transaction(&submitted, &expected, &ctx.rater)
    {
        warn!(
            network = %network,
            agent_id = %agent_id_str,
            reason = %e,
            "refusing to co-sign a Solana feedback transaction"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(FeedbackResponse {
                proof: Some(proof_report),
                success: false,
                transaction: None,
                feedback_index: None,
                error: Some(format!("{}", e)),
                network,
            }),
        )
            .into_response();
    }

    match solana_erc8004::cosign_and_send(provider.rpc_client(), provider.keypair(), submitted)
        .await
    {
        Ok(sig) => {
            info!(
                network = %network,
                tx = %sig,
                agent_id = %agent_id_str,
                rater = %ctx.rater,
                "[OK] ERC-8004 Solana feedback submitted with the RATER as author"
            );
            (
                StatusCode::OK,
                Json(FeedbackResponse {
                    proof: Some(proof_report),
                    success: true,
                    transaction: Some(crate::types::TransactionHash::Solana(sig.into())),
                    feedback_index: None,
                    error: None,
                    network,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!(network = %network, error = %e, "Solana feedback co-sign/send failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FeedbackResponse {
                    proof: Some(proof_report),
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

    let agent_id_str =
        parse_agent_id_value(&request.agent_id).unwrap_or_else(|| request.agent_id.to_string());

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
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false, "error": format!("{}", e)
                        })),
                    )
                        .into_response();
                }
            };

            // Decode seal_hash from hex string (required for Solana)
            // Accept a precomputed hash, or derive it from the original feedback so
            // callers do not have to reimplement the program's keccak256 layout.
            let seal_hash: [u8; 32] = match (&request.seal_hash, &request.original_feedback) {
                (Some(hex_str), _) => {
                    let bytes = hex::decode(hex_str.trim_start_matches("0x")).unwrap_or_default();
                    if bytes.len() != 32 {
                        return (StatusCode::BAD_REQUEST, Json(json!({
                            "success": false, "error": "sealHash must be 32 bytes (64 hex chars)"
                        }))).into_response();
                    }
                    bytes.try_into().unwrap()
                }
                (None, Some(original)) => {
                    let params = solana_erc8004::SealParams {
                        value: original.value,
                        value_decimals: original.value_decimals,
                        score: original.score,
                        feedback_file_hash: original.feedback_hash.map(|h| h.0),
                        tag1: &original.tag1,
                        tag2: &original.tag2,
                        endpoint: &original.endpoint,
                        feedback_uri: &original.feedback_uri,
                    };
                    match solana_erc8004::compute_seal_hash(&params) {
                        Some(hash) => hash,
                        None => {
                            return (StatusCode::BAD_REQUEST, Json(json!({
                                "success": false,
                                "error": "originalFeedback exceeds on-chain limits (tags 32 bytes, endpoint/uri 250 bytes, valueDecimals 0-18, score 0-100)"
                            }))).into_response();
                        }
                    }
                }
                (None, None) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "error": "Solana revocations need either sealHash or originalFeedback"
                        })),
                    )
                        .into_response();
                }
            };

            let ix = solana_erc8004::build_revoke_feedback_ix(
                &programs,
                &asset_pubkey,
                &p.keypair().pubkey(),
                request.feedback_index,
                seal_hash,
            );

            match solana_erc8004::send_erc8004_transaction(p.rpc_client(), p.keypair(), vec![ix])
                .await
            {
                Ok(sig) => {
                    info!(network = %network, tx = %sig, "ERC-8004 Solana feedback revoked");
                    (StatusCode::OK, Json(json!({
                        "success": true, "transaction": sig.to_string(), "network": network.to_string()
                    }))).into_response()
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Solana revoke failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "success": false, "error": format!("Transaction failed: {}", e)
                        })),
                    )
                        .into_response()
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
            let send_result =
                if !provider.is_eip1559() {
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
                                "success": false, "error": format!("Transaction failed: {}", e)
                            })),
                        )
                            .into_response()
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
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false, "error": format!("No provider for network {}", network)
            })),
        )
            .into_response(),
    }
}

/// `POST /feedback/response/evm/prepare`: what the RESPONDER must sign so the
/// chain records THEM as the author of the response.
///
/// The mirror of the feedback rail, for the other write the registry accepts
/// from anybody. `appendResponse` is not agent-only -- verified on-chain
/// 2026-08-18, the registry takes it from any address -- so the old
/// unauthenticated route made the FACILITATOR the `responder` on record. That
/// is the same shape as the revoke problem: a POST with no credentials makes us
/// sign. It does not destroy reputation, it ties our on-chain identity to a
/// third party's content, which is its own kind of bad.
///
/// **v4 only.** The v3 delegate accepts exactly two selectors and this is not
/// one of them, so a v3 network is refused explicitly instead of falling back to
/// the route this replaces.
#[instrument(skip_all)]
pub async fn post_prepare_relay_response<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    use crate::chain::evm::MetaEvmProvider as _;
    use crate::erc8004::relay::{self, DelegateVersion, DelegationState, RelayError};

    let request: crate::erc8004::PrepareRelayResponseRequest =
        match serde_json::from_slice(&raw_body) {
            Ok(r) => r,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Invalid request format: {}", e)})),
            )
                .into_response(),
        };
    let network = request.network;

    let bad = |code: StatusCode, msg: String| {
        (code, Json(json!({"success": false, "error": msg}))).into_response()
    };

    let Some(delegate) = relay::feedback_delegate(&network) else {
        return bad(
            StatusCode::BAD_REQUEST,
            RelayError::NoDelegate(network).to_string(),
        );
    };
    let Some(contracts) = get_contracts(&network) else {
        return bad(
            StatusCode::BAD_REQUEST,
            format!("{} does not serve ERC-8004", network),
        );
    };
    let provider_map = facilitator.provider_map();
    let Some(NetworkProvider::Evm(provider)) = provider_map.by_network(&network) else {
        return bad(
            StatusCode::BAD_REQUEST,
            format!(
                "{} is not an EVM network served by this facilitator",
                network
            ),
        );
    };

    let agent_id_str =
        parse_agent_id_value(&request.agent_id).unwrap_or_else(|| request.agent_id.to_string());
    let Ok(agent_id) = agent_id_str.parse::<u64>() else {
        return bad(
            StatusCode::BAD_REQUEST,
            format!("Invalid EVM agent ID (expected numeric): {agent_id_str}"),
        );
    };

    let version = match relay::assert_delegate_usable(
        provider.inner(),
        delegate,
        contracts.reputation_registry,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return bad(StatusCode::SERVICE_UNAVAILABLE, e.as_str().to_string()),
    };
    if version != DelegateVersion::V4 {
        return bad(
            StatusCode::BAD_REQUEST,
            RelayError::ResponseNeedsV4.as_str().to_string(),
        );
    }

    let state = match relay::delegation_state(
        provider.inner(),
        request.responder,
        delegate,
        contracts.reputation_registry,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return bad(StatusCode::SERVICE_UNAVAILABLE, e.as_str().to_string()),
    };
    if state == DelegationState::Foreign {
        return bad(
            StatusCode::BAD_REQUEST,
            RelayError::ForeignDelegation.as_str().to_string(),
        );
    }
    let delegated = state == DelegationState::Delegated;

    let account_nonce = if delegated {
        None
    } else {
        match provider
            .inner()
            .get_transaction_count(request.responder)
            .await
        {
            Ok(n) => Some(n),
            Err(_) => {
                return bad(
                    StatusCode::SERVICE_UNAVAILABLE,
                    RelayError::RpcUnavailable.as_str().to_string(),
                )
            }
        }
    };

    let deadline =
        crate::erc8004::proof::unix_now_secs().saturating_add(relay::relay_deadline_secs());
    let nonce: alloy::primitives::FixedBytes<32> = {
        use rand::RngCore as _;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        alloy::primitives::FixedBytes::from(bytes)
    };
    let chain_id = provider.chain().chain_id;
    let p = crate::erc8004::relay_v4::append_response_params(
        contracts.reputation_registry,
        agent_id,
        request.client_address,
        request.feedback_index,
        &request.response_uri,
        request.response_hash.unwrap_or_default(),
        deadline,
        nonce,
    );

    (
        StatusCode::OK,
        Json(crate::erc8004::PrepareRelayResponseResponse {
            success: true,
            delegate: Some(delegate),
            digest: Some(crate::erc8004::relay_v4::append_response_digest(
                chain_id,
                request.responder,
                &p,
            )),
            typed_data: Some(crate::erc8004::relay_v4::append_response_typed_data(
                chain_id,
                request.responder,
                &p,
            )),
            deadline: Some(deadline),
            nonce: Some(nonce),
            delegated,
            account_nonce,
            chain_id,
            error: None,
            network,
        }),
    )
        .into_response()
}

/// `POST /feedback/response/evm/submit`: relay a responder-authored response.
///
/// Same discipline as the feedback rail: the facilitator rebuilds the struct
/// from the declared parameters and requires the responder's signature to cover
/// exactly that. It does not relay a struct it was handed.
#[instrument(skip_all)]
pub async fn post_submit_relay_response<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    use crate::chain::evm::MetaEvmProvider as _;
    use crate::erc8004::relay::{self, DelegateVersion, DelegationState, RelayError};

    let request: crate::erc8004::SubmitRelayResponseRequest =
        match serde_json::from_slice(&raw_body) {
            Ok(r) => r,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Invalid request format: {}", e)})),
            )
                .into_response(),
        };
    let network = request.network;

    let refuse = |e: RelayError| {
        warn!(network = %network, reason = e.as_str(), "refusing to relay a response");
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": e.as_str(), "network": network})),
        )
            .into_response()
    };

    let Some(delegate) = relay::feedback_delegate(&network) else {
        return refuse(RelayError::NoDelegate(network));
    };
    let Some(contracts) = get_contracts(&network) else {
        return refuse(RelayError::NoDelegate(network));
    };
    let provider_map = facilitator.provider_map();
    let Some(NetworkProvider::Evm(provider)) = provider_map.by_network(&network) else {
        return refuse(RelayError::NoDelegate(network));
    };

    let agent_id_str =
        parse_agent_id_value(&request.agent_id).unwrap_or_else(|| request.agent_id.to_string());
    let Ok(agent_id) = agent_id_str.parse::<u64>() else {
        return refuse(RelayError::NoDelegate(network));
    };

    if request.deadline <= crate::erc8004::proof::unix_now_secs() {
        return refuse(RelayError::Expired);
    }

    let version = match relay::assert_delegate_usable(
        provider.inner(),
        delegate,
        contracts.reputation_registry,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return refuse(e),
    };
    if version != DelegateVersion::V4 {
        return refuse(RelayError::ResponseNeedsV4);
    }

    let chain_id = provider.chain().chain_id;
    let p = crate::erc8004::relay_v4::append_response_params(
        contracts.reputation_registry,
        agent_id,
        request.client_address,
        request.feedback_index,
        &request.response_uri,
        request.response_hash.unwrap_or_default(),
        request.deadline,
        request.nonce,
    );
    if let Err(e) = crate::erc8004::relay_v4::append_response_signature_authorises(
        chain_id,
        request.responder,
        &p,
        &request.signature,
    ) {
        return refuse(e);
    }

    let state = match relay::delegation_state(
        provider.inner(),
        request.responder,
        delegate,
        contracts.reputation_registry,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return refuse(e),
    };
    if state == DelegationState::Foreign {
        return refuse(RelayError::ForeignDelegation);
    }

    let authorization_list = if state == DelegationState::Delegated {
        match relay::nonce_already_used(provider.inner(), request.responder, request.nonce).await {
            Ok(true) => return refuse(RelayError::NonceAlreadyUsed),
            Ok(false) => {}
            Err(e) => return refuse(e),
        }
        None
    } else {
        let Some(params) = request.authorization.as_ref() else {
            return refuse(RelayError::MissingAuthorization);
        };
        let signed = relay::signed_authorization(
            alloy::primitives::U256::from(params.chain_id),
            params.address,
            params.nonce,
            params.y_parity,
            params.r,
            params.s,
        );
        if let Err(e) = relay::accept_authorization(&signed, request.responder, delegate, chain_id)
        {
            return refuse(e);
        }
        Some(vec![signed])
    };

    let calldata = crate::erc8004::relay_v4::relay_append_response_calldata(&p, &request.signature);
    // Sent TO THE RESPONDER'S ADDRESS: that is what makes the registry observe
    // them as msg.sender while our EOA only pays.
    let meta = crate::chain::evm::MetaTransaction {
        to: request.responder,
        calldata,
        confirmations: 1,
        authorization_list,
    };
    match provider
        .send_transaction_from(provider.pinned_signer(), meta)
        .await
    {
        Ok(receipt) => {
            info!(
                network = %network,
                agent_id = %agent_id_str,
                responder = %request.responder,
                tx = %receipt.transaction_hash,
                "relayed an ERC-8004 response authored by the responder"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "transaction": format!("{:#x}", receipt.transaction_hash),
                    "network": network,
                })),
            )
                .into_response()
        }
        Err(e) => {
            error!(network = %network, error = %e, "failed to relay a response");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "success": false,
                    "error": "relay_submission_failed",
                    "network": network,
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

    let agent_id_str =
        parse_agent_id_value(&request.agent_id).unwrap_or_else(|| request.agent_id.to_string());

    info!(
        network = %network,
        agent_id = %agent_id_str,
        feedback_index = request.feedback_index,
        "Appending response to ERC-8004 feedback"
    );

    // DEPRECATED authorship path, same shape as the two above: the registry
    // records `msg.sender` as the `responder`, and on this route that is US.
    // It does not destroy reputation the way the revoke route could -- it ties
    // our on-chain identity to a third party's content, which is its own kind
    // of bad, and a POST with no credentials is what triggers it.
    //
    // Warn-only, and only where the replacement actually exists: the rater-
    // authored response rail is v4-only, because the v3 delegate accepts two
    // selectors and `appendResponse` is not one of them.
    if crate::erc8004::relay::feedback_delegate(&network).is_some() {
        warn!(
            network = %network,
            agent_id = %agent_id_str,
            "[WARN] DEPRECATED: writing an EVM response authored by the FACILITATOR, \
             not by the responder. Where the delegate is v4, use \
             POST /feedback/response/evm/prepare + /feedback/response/evm/submit"
        );
    }

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
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false, "error": format!("{}", e)
                        })),
                    )
                        .into_response();
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

            let response_hash_bytes: [u8; 32] =
                request.response_hash.map(|h| h.0).unwrap_or([0u8; 32]);

            // Decode seal_hash from hex string (required for Solana)
            let seal_hash: [u8; 32] =
                match &request.seal_hash {
                    Some(hex_str) => {
                        let bytes =
                            hex::decode(hex_str.trim_start_matches("0x")).unwrap_or_default();
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
                &programs,
                &asset_pubkey,
                &client_pubkey,
                &fee_payer_pubkey,
                request.feedback_index,
                &request.response_uri,
                response_hash_bytes,
                seal_hash,
            );

            match solana_erc8004::send_erc8004_transaction(p.rpc_client(), p.keypair(), vec![ix])
                .await
            {
                Ok(sig) => {
                    info!(network = %network, tx = %sig, "ERC-8004 Solana response appended");
                    (StatusCode::OK, Json(json!({
                        "success": true, "transaction": sig.to_string(), "network": network.to_string()
                    }))).into_response()
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Solana append response failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "success": false, "error": format!("Transaction failed: {}", e)
                        })),
                    )
                        .into_response()
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
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false, "error": "Client address must be an EVM address"
                        })),
                    )
                        .into_response();
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
            let send_result =
                if !provider.is_eip1559() {
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
                                "success": false, "error": format!("Transaction failed: {}", e)
                            })),
                        )
                            .into_response()
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
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false, "error": format!("No provider for network {}", network)
            })),
        )
            .into_response(),
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
                return (StatusCode::NOT_FOUND, Json(json!({ "error": msg }))).into_response();
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
        let atom_stats_response = match solana_erc8004::read_atom_stats(
            rpc,
            &asset_pubkey,
            &programs.atom_engine,
        )
        .await
        {
            Ok(stats) => Some(AtomStatsResponse {
                trust_tier: stats.trust_tier,
                trust_tier_name: solana_erc8004::trust_tier_name(stats.trust_tier).to_string(),
                quality_score: stats.quality_score,
                loyalty_score: stats.loyalty_score,
                confidence: stats.confidence,
                risk_score: stats.risk_score,
                diversity_ratio: stats.diversity_ratio,
                min_score: stats.min_score,
                max_score: stats.max_score,
                last_score: stats.last_score,
                feedback_count: stats.feedback_count,
                last_feedback_slot: stats.last_feedback_slot,
            }),
            Err(solana_erc8004::SolanaErc8004Error::AccountNotFound(_)) => {
                // ATOM stats not initialized yet (agent has no feedback)
                None
            }
            Err(e) => {
                // A deserialization failure here is a bug, not an absent account, and
                // silently nulling it once hid a wrong AtomStats layout for months.
                error!(error = %e, "Failed to read ATOM stats, returning without ATOM data");
                None
            }
        };

        // Build summary from ATOM stats or fall back to agent account counts
        let (count, summary_value) = if let Some(ref atom) = atom_stats_response {
            (atom.feedback_count, atom.quality_score as i128)
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
            let correlation_id = uuid::Uuid::new_v4();
            error!(
                %correlation_id,
                network = %network,
                agent_id = agent_id,
                error = %e,
                "Failed to query reputation"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("reputation_query_failed (ref: {correlation_id})")
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

/// Largest number of `ownerOf` calls packed into a single Multicall3 `aggregate3`.
///
/// Sized empirically against the Base ERC-8004 registry (2026-07-24). A
/// whole-registry batch (~58.4k tokens) is rejected by the production RPC with
/// `-32003 out of gas: gas exhausted during memory expansion: 600000000`, and a
/// public Base node caps the response body at ~16.4k calls (~2.5 MB). 2,000
/// calls is roughly 6M gas and a ~320 KB response, which also fits inside the
/// tighter `eth_call` caps of small public nodes.
const OWNER_SCAN_BATCH: u64 = 2_000;

/// Hard ceiling on batches per scan, so a single lookup can never turn into an
/// unbounded RPC storm as the registry keeps growing (INC-2026-07-06).
///
/// This is a CLIFF, not a soft limit: a registry past it makes every owner
/// lookup on that chain answer 503, permanently, on the day it is crossed.
/// Measured 2026-09-01, the Base registry held **83,984** agents against the
/// 128,000 the old cap of 64 could walk -- 65% consumed, on the chain
/// Execution Market registers into. Raised to 96 (192,000 agents) so the
/// headroom is 2.3x today rather than 1.5x, and [`discover_max_agent_id`] now
/// warns from 75% so the cliff announces itself months out instead of arriving
/// as an outage.
///
/// Raising it further is not the long-term answer -- an owner index is. The
/// registries expose no owner -> agentId mapping, are not `ERC721Enumerable`,
/// and SKALE caps `eth_getLogs` at 2000 blocks, which is why there is a scan
/// here at all.
const OWNER_SCAN_MAX_BATCHES: u64 = 96;

/// Fraction of the scannable range past which a registry's size is reported as
/// a problem rather than a fact.
const OWNER_SCAN_HEADROOM_WARN_RATIO: u64 = 75;

/// The order [`scan_range_for_owner`] must examine batches in.
///
/// This is a correctness knob, not a preference, and it is decided by the
/// balance the caller already read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanOrder {
    /// Return the LOWEST matching ID. Batches run low-to-high and an earlier
    /// batch outranks a later one, because the owner may hold several agents
    /// and the contract of this lookup is the lowest.
    LowestFirst,
    /// The owner holds EXACTLY ONE token, so the only match there is is also
    /// the lowest one, and the order is free.
    ///
    /// Spent on running high-to-low. Every agent in the traffic that motivated
    /// this holds exactly one, and the ones being looked up are the ones
    /// registered most recently -- which is the top of the ID range. On the
    /// Base registry (83,984 agents on 2026-09-01) that is the difference
    /// between walking 42 batches and hitting the answer in the first.
    ///
    /// Rests on `balanceOf` being truthful, which this lookup already rests on:
    /// a zero balance is what decides not to scan at all.
    AnyMatch,
}

impl ScanOrder {
    /// The order implied by an owner's balance.
    fn for_balance(balance: alloy::primitives::U256) -> Self {
        if balance == alloy::primitives::U256::from(1) {
            Self::AnyMatch
        } else {
            Self::LowestFirst
        }
    }
}

/// How long a scan hint stays usable.
///
/// A wrong hint costs latency, never correctness, so this can be long. It is
/// refreshed on every successful `AnyMatch` scan, so an active registry keeps
/// its hint warm and a quiet one simply pays a cold scan once.
const SCAN_HINT_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// How many distinct hot batches to remember per registry.
///
/// One is not enough, and that is measured rather than supposed. Base's owner
/// traffic runs in TWO clusters -- agents around 18,800 and around 58,600, which
/// are batches 10 and 30 of 42 -- so a single hint ping-pongs between them and
/// every alternation pays the full expansion. Observed in production
/// 2026-09-01 under 2.4.0: ten of twelve Base lookups answered in 0.47-1.34s and
/// two in 4.1-5.2s, and the slow ones were exactly the ones that followed a
/// match from the other cluster.
///
/// Tied to [`OWNER_SCAN_WAVE`] on purpose: remembering as many batches as a wave
/// can issue concurrently is what puts every hot cluster in the FIRST wave.
/// Remembering more would not help -- they could not be probed together anyway.
const SCAN_HINT_SLOTS: usize = OWNER_SCAN_WAVE;

/// The batch an agent ID falls in, for a scan that starts at 1.
///
/// Only used to keep the remembered hints one-per-batch: two agents in the same
/// batch are the same hint, because probing that batch finds both.
fn hint_batch(agent_id: u64) -> u64 {
    agent_id / OWNER_SCAN_BATCH
}

/// Where the last `AnyMatch` scan found a match, per `(network, registry)`.
///
/// The point of this cache is to stop GUESSING where agents live.
///
/// The first version of the reordering assumed "recently registered agents have
/// high IDs" and swept high-to-low. Measured against three hours of production
/// logs that holds on eight of nine networks -- and fails on the one that
/// matters. Base carries the most owner lookups (820 in 3h), is the only
/// registry big enough for the order to matter (42 batches), and its median
/// resolved agent sits at 18,897 of 83,984: batch 10 of 42, which high-to-low
/// reaches almost last. That put `/identity/base/owner/...` at 4-9.6s while
/// every other network answered in one wave.
///
/// The assumption was plausible, was never checked against the traffic, and
/// looked right in the two addresses that happened to get sampled -- the same
/// shape as the `totalSupply()` defect this module was fixing in the first
/// place. So this does not assume: it remembers what recent lookups actually
/// found, and it remembers SEVERAL, because real traffic is not one cluster.
///
/// Most recent first, at most one entry per batch, capped at
/// [`SCAN_HINT_SLOTS`].
#[allow(clippy::type_complexity)]
static SCAN_HINT_CACHE: Lazy<
    dashmap::DashMap<
        (crate::network::Network, alloy::primitives::Address),
        (Vec<u64>, std::time::Instant),
    >,
> = Lazy::new(dashmap::DashMap::new);

/// The agent IDs recent successful `AnyMatch` scans of this registry found,
/// most recent first. Empty when nothing is remembered or the entry went stale.
fn scan_hints(network: crate::network::Network, registry: alloy::primitives::Address) -> Vec<u64> {
    match SCAN_HINT_CACHE.get(&(network, registry)) {
        Some(entry) if entry.1.elapsed() < SCAN_HINT_TTL => entry.0.clone(),
        _ => Vec::new(),
    }
}

/// Record where a scan found its match, so the next ones start there.
///
/// Keeps the most recent hit per batch and drops the oldest beyond the cap, so
/// a registry whose traffic moves follows it instead of accumulating history.
fn store_scan_hint(
    network: crate::network::Network,
    registry: alloy::primitives::Address,
    agent_id: u64,
) {
    let now = std::time::Instant::now();
    SCAN_HINT_CACHE
        .entry((network, registry))
        .and_modify(|slot| {
            if slot.1.elapsed() >= SCAN_HINT_TTL {
                slot.0.clear();
            }
            slot.0
                .retain(|held| hint_batch(*held) != hint_batch(agent_id));
            slot.0.insert(0, agent_id);
            slot.0.truncate(SCAN_HINT_SLOTS);
            slot.1 = now;
        })
        .or_insert_with(|| (vec![agent_id], now));
}

/// The order to examine batches in for an `AnyMatch` scan.
///
/// Returns indices into `ranges`, and it is ALWAYS a permutation of them: every
/// batch appears exactly once. That property is the one that cannot break --
/// a missing index silently skips a slice of the registry, and a skipped agent
/// is answered as "not registered", which callers persist.
///
/// The batches holding the `hints` go first, in the order given, so every
/// remembered cluster is probed in the first wave. The rest follow by distance
/// to the NEAREST hint, which keeps a lookup that lands between two clusters
/// cheap instead of making it walk from one end.
///
/// With no hints at all the batches alternate from the high end and the low
/// end, so neither extreme is the pathological case: the worst a hintless scan
/// can cost is half the registry rather than all of it.
fn any_match_batch_order(ranges: &[(u64, u64)], hints: &[u64]) -> Vec<usize> {
    let n = ranges.len();
    if n == 0 {
        return Vec::new();
    }

    // The batch each hint falls in. A hint outside the range being scanned --
    // the registry grew, or this is the tail rescan -- clamps to the near end
    // rather than being dropped.
    let mut seeds: Vec<usize> = Vec::new();
    for hint in hints {
        let index = ranges
            .iter()
            .position(|(first, last)| hint >= first && hint <= last)
            .unwrap_or_else(|| if *hint < ranges[0].0 { 0 } else { n - 1 });
        if !seeds.contains(&index) {
            seeds.push(index);
        }
    }

    if seeds.is_empty() {
        // No hint: interleave the two ends, high first. High first because eight
        // of the nine measured networks keep their live agents at the top of the
        // registry -- but interleaved, so the ninth is not paying for that.
        let mut order = Vec::with_capacity(n);
        let mut low = 0usize;
        let mut high = n - 1;
        loop {
            order.push(high);
            if order.len() == n {
                break;
            }
            order.push(low);
            if order.len() == n {
                break;
            }
            low += 1;
            high -= 1;
            if low > high {
                break;
            }
        }
        return order;
    }

    // Seeds first, then the complement sorted by distance to the NEAREST seed.
    // Building the tail as the complement is what makes the permutation
    // property structural rather than something a loop has to get right.
    let mut order = seeds.clone();
    let mut rest: Vec<usize> = (0..n).filter(|i| !seeds.contains(i)).collect();
    rest.sort_by_key(|i| {
        let nearest = seeds
            .iter()
            .map(|seed| i.abs_diff(*seed))
            .min()
            .unwrap_or(usize::MAX);
        (nearest, *i)
    });
    order.extend(rest);
    order
}
/// Batches issued CONCURRENTLY per wave by [`scan_range_for_owner`].
///
/// The scan stops at the first match, so a wave can do work a strictly serial
/// walk would have skipped -- at most `this - 1` wasted batches. That is the
/// whole trade, and it is worth taking: the Base registry held between 65,536
/// and 262,144 agents on 2026-09-01, which is up to 50 batches, and fifty
/// SERIAL Multicall3 round trips is the 3-9s Base spent inside this function
/// while the rest of the facilitator answered in 33ms.
///
/// Deliberately small. Widening it multiplies the instantaneous load one
/// request puts on the shared RPC budget, and exhausting that budget is what
/// starved `/settle` in INC-2026-07-06. Four cuts the tail by ~4x while keeping
/// the burst modest, and the bound cache means most lookups never scan cold at
/// all.
const OWNER_SCAN_WAVE: usize = 4;

/// How long a successful owner -> agent ID resolution stays cached.
///
/// A cold resolution costs tens of RPC calls, and Execution Market performs one
/// per signed request: 6,965 lookups in 72h exhausted the shared Base RPC
/// budget and produced 258 `-32007` rate limits, which in turn starved
/// `/settle` (INC-2026-07-06). Only positive results are cached — a negative
/// answer flips as soon as the agent registers, so caching it would be wrong.
const OWNER_LOOKUP_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Cache of resolved `(network, registry, owner)` -> `agentId`.
///
/// The network is part of the key because the ERC-8004 registries are deployed
/// at the same deterministic address on every chain, so `(registry, owner)`
/// alone would serve a Base agent ID for a Polygon lookup.
#[allow(clippy::type_complexity)]
static OWNER_LOOKUP_CACHE: Lazy<
    dashmap::DashMap<
        (
            crate::network::Network,
            alloy::primitives::Address,
            alloy::primitives::Address,
        ),
        (u64, std::time::Instant),
    >,
> = Lazy::new(dashmap::DashMap::new);

/// Classify an `eth_call` failure.
///
/// Returns `true` only when the node actually executed the call and the
/// contract reverted — which, for `ownerOf`, means the token does not exist.
///
/// Everything else (rate limits, out-of-gas, malformed provider errors,
/// transport failures) carries **no verdict** about the token and must return
/// `false`. Conflating the two is what silently truncated the scan range and
/// let `POST /register` mint duplicate agent NFTs: an unrecognised error is
/// therefore treated as inconclusive, never as "token absent".
pub(crate) fn is_execution_revert(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("execution reverted")
        // `ERC721NonexistentToken(uint256)` selector (OpenZeppelin v5), returned
        // as revert data by the ERC-8004 registries.
        || lower.contains("0x7e273289")
        || lower.contains("nonexistent")
        || lower.contains("invalid token")
}

/// Outcome of one `ownerOf` probe carried inside a Multicall3 batch.
///
/// `aggregate3` with `allowFailure: true` reports a reverting sub-call as
/// `success: false`, which for `ownerOf` means the token does not exist. A
/// batch that never executed at all fails at the TOP level and surfaces as
/// `Err` from [`multicall_owner_of`] -- so "absent" and "no verdict" stay
/// distinct here exactly as they do in [`is_execution_revert`], and an
/// unreachable RPC can never be read as proof that a token is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenProbe {
    /// The call executed and returned this owner.
    Owned(alloy::primitives::Address),
    /// The call reverted: the token does not exist.
    Absent,
}

/// Batch `ownerOf` over an arbitrary set of agent IDs in ONE Multicall3 call.
///
/// The single RPC primitive behind both jobs this module has -- finding the
/// registry's highest ID ([`discover_max_agent_id`]) and finding an owner's
/// token ([`scan_range_for_owner`]) -- so the batch limits, the decode
/// discipline and the absent/no-verdict distinction are written once and
/// cannot drift between them.
///
/// IDs need not be contiguous: the bound search probes a sparse ladder through
/// the same path the dense scan uses.
async fn multicall_owner_of(
    provider: &crate::chain::evm::InnerProvider,
    registry: alloy::primitives::Address,
    ids: &[u64],
) -> Result<Vec<TokenProbe>, String> {
    use alloy::providers::bindings::IMulticall3;
    use alloy::providers::MULTICALL3_ADDRESS;
    use alloy::sol_types::SolCall;

    if ids.is_empty() {
        return Ok(Vec::new());
    }
    if ids.len() as u64 > OWNER_SCAN_BATCH {
        return Err(format!(
            "multicall of {} ownerOf calls exceeds the measured batch cap of {OWNER_SCAN_BATCH}",
            ids.len()
        ));
    }

    let calls: Vec<IMulticall3::Call3> = ids
        .iter()
        .map(|id| IMulticall3::Call3 {
            target: registry,
            allowFailure: true,
            callData: IIdentityRegistry::ownerOfCall {
                agentId: alloy::primitives::U256::from(*id),
            }
            .abi_encode()
            .into(),
        })
        .collect();

    let encoded = IMulticall3::aggregate3Call { calls }.abi_encode();
    let tx = alloy::rpc::types::TransactionRequest::default()
        .to(MULTICALL3_ADDRESS)
        .input(alloy::rpc::types::TransactionInput::new(encoded.into()));

    let first = ids[0];
    let last = ids[ids.len() - 1];
    let raw_result = provider.call(tx).await.map_err(|e| {
        format!(
            "Multicall3 ownerOf batch of {} ({first}..={last}) failed: {e}",
            ids.len()
        )
    })?;

    let results = IMulticall3::aggregate3Call::abi_decode_returns(&raw_result)
        .map_err(|e| format!("Failed to decode multicall results: {e}"))?;

    // A short return array would silently shift every ID-to-result mapping
    // below it, which is how a scan reports the WRONG agent rather than none.
    if results.len() != ids.len() {
        return Err(format!(
            "Multicall3 returned {} results for {} ownerOf calls ({first}..={last})",
            results.len(),
            ids.len()
        ));
    }

    Ok(results
        .iter()
        .map(|r| {
            if r.success && r.returnData.len() >= 32 {
                // ownerOf returns an abi-encoded address: 32 bytes, left-padded.
                TokenProbe::Owned(alloy::primitives::Address::from_slice(
                    &r.returnData[12..32],
                ))
            } else {
                TokenProbe::Absent
            }
        })
        .collect())
}

/// `true` for every probe that came back with an owner.
fn probes_present(probes: &[TokenProbe]) -> Vec<bool> {
    probes
        .iter()
        .map(|p| matches!(p, TokenProbe::Owned(_)))
        .collect()
}

/// Ceiling of the exponential ladder used to bracket the highest agent ID:
/// `2^24`, about 16.7M agents.
///
/// The whole ladder is probed in ONE Multicall3 round trip, so a higher ceiling
/// costs nothing per request -- it only widens the range the search can
/// describe. Running off the end is an explicit `Err`, never a truncated bound:
/// the sequential probe this replaced stopped doubling at 1,000,000 and then
/// binary-searched inside the range it had already left, which answers with a
/// maximum far below the real one and turns every agent above it into a 404.
const BOUND_LADDER_MAX_EXP: u32 = 24;

/// Probe points spent per refinement round.
///
/// Each round divides the remaining span by `this + 1`, so 1,000 points take
/// the full `2^24` ladder span to ~16.7k, then to ~16, then to the exact
/// answer: **at most four round trips for any registry the ladder describes**.
///
/// What it replaces: an exponential probe plus binary search over single
/// `eth_call`s, ~28 of them STRICTLY IN SERIES on the celo registry. Measured
/// 2026-09-01 at ~400ms per call against the production RPC, that was 11.2s per
/// cold lookup and it held the p99 of the entire facilitator at 11.4s for
/// sixteen hours.
///
/// Stays under [`OWNER_SCAN_BATCH`], the gas and response-size limit measured
/// against the production RPCs.
const BOUND_SEARCH_PROBES_PER_ROUND: u64 = 1_000;

/// Hard cap on refinement rounds. Four suffice for the whole ladder; the cap
/// exists so a registry that answers inconsistently cannot spin.
const BOUND_SEARCH_MAX_ROUNDS: u32 = 8;

/// How long a discovered registry bound stays cached, per `(network, registry)`.
///
/// The highest agent ID only ever GROWS, so a stale entry can only be too LOW,
/// and too low is self-healing: the scan misses, [`resolve_first_token_by_owner`]
/// re-derives the bound, scans the tail it could not see, and re-caches the
/// fresh value. The TTL is therefore a backstop rather than the correctness
/// mechanism, and it trades against one extra discovery -- never a wrong answer.
const REGISTRY_BOUND_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// Cache of `(network, registry)` -> highest known agent ID.
///
/// This is the entry that was missing. Every cold owner lookup used to
/// re-derive the same registry-wide number from scratch, in series, per
/// request -- a value that is identical for every caller and changes only when
/// somebody registers a new agent.
///
/// Keyed by network as well as registry because the ERC-8004 registries are
/// deployed at the same deterministic address on every chain, so `registry`
/// alone would serve Base's bound for a Celo lookup.
#[allow(clippy::type_complexity)]
static REGISTRY_BOUND_CACHE: Lazy<
    dashmap::DashMap<
        (crate::network::Network, alloy::primitives::Address),
        (u64, std::time::Instant),
    >,
> = Lazy::new(dashmap::DashMap::new);

/// Fresh cached bound for a registry, if any.
fn cached_registry_bound(
    network: crate::network::Network,
    registry: alloy::primitives::Address,
) -> Option<u64> {
    let entry = REGISTRY_BOUND_CACHE.get(&(network, registry))?;
    let (max_id, cached_at) = *entry;
    (cached_at.elapsed() < REGISTRY_BOUND_TTL).then_some(max_id)
}

/// Record a freshly discovered bound, keeping the highest value seen.
///
/// Monotone on purpose: the maximum only grows, so a concurrent request that
/// discovered a higher bound must not be walked back by one that started
/// earlier and finished later.
fn store_registry_bound(
    network: crate::network::Network,
    registry: alloy::primitives::Address,
    max_id: u64,
) {
    REGISTRY_BOUND_CACHE
        .entry((network, registry))
        .and_modify(|slot| {
            if max_id >= slot.0 {
                *slot = (max_id, std::time::Instant::now());
            }
        })
        .or_insert((max_id, std::time::Instant::now()));
}

/// The exponential ladder: `1, 2, 4, ... 2^BOUND_LADDER_MAX_EXP`.
fn bound_ladder_points() -> Vec<u64> {
    (0..=BOUND_LADDER_MAX_EXP).map(|e| 1u64 << e).collect()
}

/// Bracket the highest existing agent ID from one ladder probe.
///
/// Returns `(lo, hi)` with `lo` present and `hi` absent, so the true maximum
/// lies in `[lo, hi)`.
///
/// - `Ok(None)`: not one ladder point exists -- an empty registry.
/// - `Err`: every point up to the ceiling exists, so the registry is larger
///   than this search can describe. Reported, never truncated.
///
/// Takes the HIGHEST present point and the LOWEST absent point above it rather
/// than stopping at the first absent one, so a burned agent that happens to sit
/// on a power of two cannot cut the bracket short. Free -- it is the same
/// probe data, read more carefully.
fn bracket_from_ladder(points: &[u64], present: &[bool]) -> Result<Option<(u64, u64)>, String> {
    if points.len() != present.len() {
        return Err(format!(
            "ladder probe returned {} results for {} points",
            present.len(),
            points.len()
        ));
    }

    let lo = points
        .iter()
        .zip(present)
        .filter(|(_, ok)| **ok)
        .map(|(p, _)| *p)
        .max();
    let Some(lo) = lo else {
        return Ok(None);
    };

    let hi = points
        .iter()
        .zip(present)
        .filter(|(p, ok)| !**ok && **p > lo)
        .map(|(p, _)| *p)
        .min();

    match hi {
        Some(hi) => Ok(Some((lo, hi))),
        None => Err(format!(
            "registry holds agent {lo}, at or beyond the 2^{BOUND_LADDER_MAX_EXP} ceiling this \
             search can bracket; the bound is unknown rather than equal to the ceiling"
        )),
    }
}

/// Evenly spaced probe points strictly inside `(lo, hi)`.
///
/// When the gap holds no more than `k` candidates every one of them is
/// returned, so the round that consumes them is exact.
fn refine_points(lo: u64, hi: u64, k: u64) -> Vec<u64> {
    if hi <= lo + 1 || k == 0 {
        return Vec::new();
    }
    let candidates = hi - lo - 1;
    if candidates <= k {
        return (lo + 1..hi).collect();
    }
    let span = hi - lo;
    let mut points: Vec<u64> = (1..=k)
        .map(|i| lo + (span.saturating_mul(i)) / (k + 1))
        .filter(|p| *p > lo && *p < hi)
        .collect();
    points.dedup();
    points
}

/// Shrink `(lo, hi)` with one round of probe results.
///
/// Absent points are only allowed to lower `hi` when they sit ABOVE the new
/// `lo`; without that guard a hole below a confirmed agent would invert the
/// bracket.
fn narrow_bracket(lo: u64, hi: u64, points: &[u64], present: &[bool]) -> (u64, u64) {
    let new_lo = points
        .iter()
        .zip(present)
        .filter(|(_, ok)| **ok)
        .map(|(p, _)| *p)
        .fold(lo, u64::max);
    let new_hi = points
        .iter()
        .zip(present)
        .filter(|(p, ok)| !**ok && **p > new_lo)
        .map(|(p, _)| *p)
        .fold(hi, u64::min);
    (new_lo, new_hi)
}

/// Highest existing agent ID in the registry.
///
/// One Multicall3 round trip brackets the answer with an exponential ladder;
/// each further round trip probes [`BOUND_SEARCH_PROBES_PER_ROUND`] points
/// inside the bracket. Four round trips cover the entire ladder range.
///
/// `totalSupply()` is deliberately NOT consulted. It is in the ABI, but it
/// **reverts on every ERC-8004 registry actually deployed** -- verified
/// on-chain 2026-09-01 against celo and base, where `supportsInterface`
/// for `ERC721Enumerable` also answers false. A previous revision put that call
/// first and treated the sequential probe as the fallback; because the call
/// always failed, the "fallback" was the only path that ever ran and the
/// optimisation was a no-op in production for its entire life. Reintroducing it
/// would buy at most one round trip off a cold lookup while restoring exactly
/// that failure mode. Run `scripts/erc8004_registry_capabilities.py` before
/// believing otherwise.
///
/// ASSUMPTION, and not a new one: agent IDs run contiguously from 1. Every
/// bound search that has ever run here has assumed it. Where it does not hold,
/// [`resolve_first_token_by_owner`] refuses to answer "not registered" rather
/// than guessing.
async fn discover_max_agent_id(
    provider: &crate::chain::evm::InnerProvider,
    registry: alloy::primitives::Address,
) -> Result<u64, String> {
    let ladder = bound_ladder_points();
    let probes = multicall_owner_of(provider, registry, &ladder).await?;
    let (mut lo, mut hi) = match bracket_from_ladder(&ladder, &probes_present(&probes))? {
        Some(bracket) => bracket,
        None => {
            // Callers only reach a scan with a non-zero balance, so an
            // apparently empty registry contradicts that balance and is not a
            // clean "owns nothing" answer.
            return Err("registry probe found no tokens despite a non-zero balance".to_string());
        }
    };

    let mut rounds: u32 = 1;
    while hi > lo + 1 {
        if rounds >= BOUND_SEARCH_MAX_ROUNDS {
            return Err(format!(
                "registry bound search did not converge within {BOUND_SEARCH_MAX_ROUNDS} rounds \
                 (bracket {lo}..{hi})"
            ));
        }
        let points = refine_points(lo, hi, BOUND_SEARCH_PROBES_PER_ROUND);
        if points.is_empty() {
            break;
        }
        let probes = multicall_owner_of(provider, registry, &points).await?;
        let (next_lo, next_hi) = narrow_bracket(lo, hi, &points, &probes_present(&probes));
        rounds += 1;
        if next_lo == lo && next_hi == hi {
            return Err(format!(
                "registry bound search stalled at {lo}..{hi} after {rounds} rounds"
            ));
        }
        lo = next_lo;
        hi = next_hi;
    }

    let scannable = OWNER_SCAN_MAX_BATCHES * OWNER_SCAN_BATCH;
    if lo * 100 >= scannable * OWNER_SCAN_HEADROOM_WARN_RATIO {
        // Deliberately WARN, and deliberately not silent. The last time this
        // module took a quiet path nobody could see, it was a `debug!` in a
        // service running at `info` -- so the one line that would have revealed
        // an optimisation was a no-op never printed, and the p99 sat at 11.4s
        // for sixteen hours.
        warn!(
            max_agent_id = lo,
            scannable,
            "Registry is approaching the size this scan can walk; past it every owner \
             lookup on this chain answers 503. An owner index is the fix, not a \
             bigger cap."
        );
    } else {
        debug!(
            max_agent_id = lo,
            round_trips = rounds,
            "Registry bound discovered"
        );
    }
    Ok(lo)
}

/// Split `first..=last` into the batches one scan issues.
///
/// Pure, and split out because a gap or an overlap here does not fail loudly --
/// it silently skips an agent, and a skipped agent is answered as "not
/// registered". The tests assert the ranges tile the span exactly.
fn scan_batch_ranges(first: u64, last: u64) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    if first > last {
        return ranges;
    }
    let mut start = first;
    while start <= last {
        let end = (start + OWNER_SCAN_BATCH - 1).min(last);
        ranges.push((start, end));
        start = end + 1;
    }
    ranges
}

/// Scan `ownerOf(first..=last)` in bounded Multicall3 batches and return the
/// lowest ID owned by `target`.
///
/// `Err` means the scan reached no verdict; it must never be read as a clean
/// "owns nothing".
async fn scan_range_for_owner(
    provider: &crate::chain::evm::InnerProvider,
    registry: alloy::primitives::Address,
    target: alloy::primitives::Address,
    first: u64,
    last: u64,
    order: ScanOrder,
    hints: &[u64],
) -> Result<Option<u64>, String> {
    if first > last {
        return Ok(None);
    }

    // One source of truth for how many batches this span costs: the same
    // function that produces them.
    let mut ranges = scan_batch_ranges(first, last);
    if ranges.len() as u64 > OWNER_SCAN_MAX_BATCHES {
        return Err(format!(
            "registry too large to scan: {first}..={last} needs {} batches \
             (cap {OWNER_SCAN_MAX_BATCHES})",
            ranges.len()
        ));
    }

    // Under `LowestFirst` the waves preserve "lowest ID wins": every batch of an
    // earlier wave is fully examined before a later wave runs, and within a wave
    // the minimum matching ID is taken. Concurrency changes how long the answer
    // takes, never which answer it is.
    //
    // Under `AnyMatch` the owner holds exactly one token, so there is only one
    // match to find and the order cannot change the answer -- only how soon it
    // arrives. So the batches are reordered to start where the last match was
    // actually found. See [`any_match_batch_order`] for why that is measured
    // rather than assumed.
    if order == ScanOrder::AnyMatch {
        let sequence = any_match_batch_order(&ranges, hints);
        debug_assert_eq!(
            sequence.len(),
            ranges.len(),
            "the batch order must be a permutation: a missing batch silently skips agents"
        );
        ranges = sequence.into_iter().map(|i| ranges[i]).collect();
    }

    for wave in ranges.chunks(OWNER_SCAN_WAVE) {
        let mut tasks = tokio::task::JoinSet::new();
        for (batch_first, batch_last) in wave.iter().copied() {
            let provider = provider.clone();
            tasks.spawn(async move {
                let ids: Vec<u64> = (batch_first..=batch_last).collect();
                multicall_owner_of(&provider, registry, &ids)
                    .await
                    .map(|probes| (ids, probes))
            });
        }

        let mut lowest_match: Option<u64> = None;
        while let Some(joined) = tasks.join_next().await {
            // A batch that reached no verdict poisons the WHOLE scan: a match
            // could have been in it, so what is left is not a clean miss.
            // Dropping the JoinSet here aborts the siblings still in flight.
            let (ids, probes) =
                joined.map_err(|e| format!("owner scan batch did not complete: {e}"))??;

            for (i, probe) in probes.iter().enumerate() {
                if let TokenProbe::Owned(owner) = probe {
                    if *owner == target {
                        // IDs ascend within a batch, so the first hit is that
                        // batch's lowest.
                        let id = ids[i];
                        lowest_match = Some(lowest_match.map_or(id, |best| best.min(id)));
                        break;
                    }
                }
            }
        }

        if let Some(agent_id) = lowest_match {
            return Ok(Some(agent_id));
        }
    }

    Ok(None)
}

/// Resolve the first (lowest) token ID owned by `target` in an ERC-721 contract.
///
/// `known_balance` is the `balanceOf(target)` the caller has ALREADY read, and
/// it is what makes the three outcomes separable:
///
/// - `Ok(Some(id))` -- found.
/// - `Ok(None)` -- scanned cleanly and the address owns nothing. Only a
///   truthful answer when `known_balance` is zero.
/// - `Err` -- the scan reached no verdict. Callers must not treat this as
///   proof that an address owns nothing.
///
/// **A non-zero balance with nothing found is a CONTRADICTION, not a miss.**
/// The registry says the address holds tokens; the scan could not attribute
/// one. That means the RANGE was wrong, and answering "owns nothing" is the
/// most expensive wrong answer this codebase has: callers persist a 404 as
/// "not registered" and stop asking (INC-2026-07-21), and `POST /register`
/// reads it as permission to mint -- handing a duplicate identity to someone
/// who already has one, and growing the registry that made the scan fail.
/// So it is reported as no verdict, loudly, and the caller retries.
///
/// Cost of a cold lookup: one bound discovery (up to four Multicall3 round
/// trips, or zero when the registry bound is cached) plus one scan batch per
/// 2,000 agents.
async fn resolve_first_token_by_owner(
    provider: &crate::chain::evm::InnerProvider,
    network: crate::network::Network,
    registry: alloy::primitives::Address,
    target: alloy::primitives::Address,
    known_balance: alloy::primitives::U256,
) -> Result<Option<u64>, String> {
    // Serve a fresh cached resolution before spending any RPC budget.
    if let Some(entry) = OWNER_LOOKUP_CACHE.get(&(network, registry, target)) {
        let (agent_id, cached_at) = *entry;
        if cached_at.elapsed() < OWNER_LOOKUP_TTL {
            debug!(agent_id, %target, "Owner lookup served from cache");
            return Ok(Some(agent_id));
        }
    }

    // Step 1: the upper bound of the range to scan. Registry-wide and identical
    // for every caller, so it is cached across them.
    let cached_bound = cached_registry_bound(network, registry);
    let max_id = match cached_bound {
        Some(cached) => cached,
        None => {
            let discovered = discover_max_agent_id(provider, registry).await?;
            store_registry_bound(network, registry, discovered);
            discovered
        }
    };

    // How far the scan actually reached. Step 3 can push this above `max_id`,
    // and the answer in step 4 has to report the range that was really walked
    // -- an error that names the wrong range sends the reader to the wrong
    // place.
    let mut scanned_to = max_id;

    // Step 2: scan ascending, stopping at the first match so the lowest ID is
    // returned and the common case stays cheap.
    let order = ScanOrder::for_balance(known_balance);
    let hints = scan_hints(network, registry);
    if let Some(agent_id) =
        scan_range_for_owner(provider, registry, target, 1, max_id, order, &hints).await?
    {
        OWNER_LOOKUP_CACHE.insert(
            (network, registry, target),
            (agent_id, std::time::Instant::now()),
        );
        store_scan_hint(network, registry, agent_id);
        return Ok(Some(agent_id));
    }

    // Step 3: a cached bound can only be too LOW -- an agent registered since
    // it was written sits above it. Re-derive and scan only the tail the first
    // pass could not see. Skipped when the bound was just discovered, because
    // repeating a deterministic search returns the same number.
    if cached_bound.is_some() {
        let fresh = discover_max_agent_id(provider, registry).await?;
        store_registry_bound(network, registry, fresh);
        if fresh > max_id {
            scanned_to = fresh;
            debug!(
                stale_bound = max_id,
                fresh_bound = fresh,
                "Cached registry bound was stale; scanning the tail"
            );
            if let Some(agent_id) =
                scan_range_for_owner(provider, registry, target, max_id + 1, fresh, order, &hints)
                    .await?
            {
                OWNER_LOOKUP_CACHE.insert(
                    (network, registry, target),
                    (agent_id, std::time::Instant::now()),
                );
                store_scan_hint(network, registry, agent_id);
                return Ok(Some(agent_id));
            }
        }
    }

    // Step 4: the range is exhausted. Which answer that is depends entirely on
    // the balance the caller already read.
    if known_balance > alloy::primitives::U256::ZERO {
        warn!(
            network = %network,
            owner = %target,
            balance = %known_balance,
            scanned_to,
            "Registry balance contradicts the ownerOf scan: answering no-verdict rather than a \
             false 'not registered'"
        );
    }
    exhausted_scan_outcome(target, known_balance, scanned_to)
}

/// The answer when the scan ran to the end of the range and found nothing.
///
/// Pure, and split out for the same reason [`owner_lookup_response`] is: the
/// distinction it encodes must be testable without a chain behind it.
///
/// - Zero balance -> `Ok(None)`, a truthful "owns nothing".
/// - Non-zero balance -> `Err`. The registry says the address holds tokens and
///   the scan could not attribute one, so the RANGE was wrong. Reporting that
///   as "owns nothing" is how a transient scan defect becomes a permanent wrong
///   answer on the caller's side (INC-2026-07-21) and, on `POST /register`,
///   permission to mint a duplicate identity.
fn exhausted_scan_outcome(
    target: alloy::primitives::Address,
    known_balance: alloy::primitives::U256,
    scanned_to: u64,
) -> Result<Option<u64>, String> {
    if known_balance > alloy::primitives::U256::ZERO {
        return Err(format!(
            "balanceOf({target}) is {known_balance} but no agent in 1..={scanned_to} is owned by \
             it; the scan range is wrong, so this is not proof that the address owns nothing"
        ));
    }
    Ok(None)
}

/// Path parameters for identity-by-owner query
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OwnerIdentityPathParams {
    pub network: String,
    pub address: String,
}

/// `GET /identity/:network/owner/:address`: Resolve agent ID by owner wallet address.
///
/// Scans `Registered` events filtered by owner, then verifies current ownership via `ownerOf()`.
/// Returns the first (lowest) agent ID still owned by the address.
///
/// # Example
/// ```text
/// GET /identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15
/// ```
#[instrument(skip_all)]
pub async fn get_identity_by_owner<A>(
    State(facilitator): State<A>,
    Path(params): Path<OwnerIdentityPathParams>,
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
                Json(json!({ "error": format!("Invalid network: {}", params.network) })),
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

    // ---- Solana branch: scan AgentAccount PDAs filtered by owner ----
    if solana_erc8004::is_solana_erc8004_supported(&network) {
        return get_identity_by_owner_solana(facilitator, network, &params.address).await;
    }

    let owner_address: alloy::primitives::Address = match params.address.parse() {
        Ok(a) => a,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("Invalid address: {}", params.address) })),
            )
                .into_response();
        }
    };

    info!(network = %network, owner = %owner_address, "Resolving agent ID by owner");

    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("No ERC-8004 contracts for {}", network) })),
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
                Json(json!({ "error": format!("No EVM provider for {}", network) })),
            )
                .into_response();
        }
    };

    let identity_registry =
        IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone());

    // Check balance first — quick rejection
    let balance = match identity_registry.balanceOf(owner_address).call().await {
        Ok(b) => b,
        Err(e) => {
            let correlation_id = uuid::Uuid::new_v4();
            error!(%correlation_id, error = %e, "Failed to query balanceOf");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("balance_query_failed (ref: {correlation_id})") })),
            )
                .into_response();
        }
    };

    if balance == alloy::primitives::U256::ZERO {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("Address {} does not own any agent on {}", owner_address, network),
                "balance": 0
            })),
        )
            .into_response();
    }

    // Resolve token ID using Multicall3 batched ownerOf() calls, split into
    // bounded batches so the request cost does not grow with registry size.
    // Multicall3 is at the canonical address
    // 0xcA11bde05977b3631167028862bE2a173976CA11 on every supported chain.
    let outcome = match resolve_first_token_by_owner(
        provider.inner(),
        network,
        contracts.identity_registry,
        owner_address,
        balance,
    )
    .await
    {
        Ok(Some(agent_id)) => {
            let uri = identity_registry
                .tokenURI(alloy::primitives::U256::from(agent_id))
                .call()
                .await
                .unwrap_or_default();
            info!(network = %network, agent_id = agent_id, owner = %owner_address, "Resolved agent by owner");
            Ok(Some((agent_id, uri)))
        }
        Ok(None) => {
            info!(network = %network, owner = %owner_address, "Owner holds no agent in registry");
            Ok(None)
        }
        Err(e) => {
            warn!(network = %network, owner = %owner_address, error = %e, "Owner lookup inconclusive");
            Err(e)
        }
    };

    owner_lookup_response(network, owner_address, &balance.to_string(), outcome).into_response()
}

/// Map an owner-scan outcome onto its HTTP response.
///
/// Pure, and split out of [`get_identity_by_owner`] so the one distinction that
/// must never collapse is testable without a chain behind it:
///
/// - `Ok(None)` is **404** -- "this address owns no agent".
/// - `Err` is **503 + `retryable: true`** -- "the lookup reached no verdict".
///
/// Callers persist a 404 as "not registered" and stop asking, so answering 404
/// to what was really a transient RPC failure turns our outage into a permanent
/// wrong answer -- and on the registration path it mints a duplicate agent for
/// someone who already has one (INC-2026-07-21).
fn owner_lookup_response(
    network: crate::network::Network,
    owner: alloy::primitives::Address,
    balance: &str,
    outcome: Result<Option<(u64, String)>, String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match outcome {
        Ok(Some((agent_id, uri))) => (
            StatusCode::OK,
            Json(json!({
                "agentId": agent_id,
                "owner": format!("{owner}"),
                "agentUri": uri,
                "network": network.to_string(),
                "balance": balance
            })),
        ),
        // Scanned cleanly: the balance is held by tokens we could not attribute,
        // which for a well-formed registry means there is nothing to return.
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("Address {owner} does not own any agent on {network}"),
                "balance": balance
            })),
        ),
        // The scan never reached a verdict. This must NOT be a 404: 503 tells
        // the client to retry instead of recording a permanent absence.
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": format!("Could not determine agent ID for {owner} on {network}: {e}"),
                "retryable": true,
                "balance": balance
            })),
        ),
    }
}

/// Solana half of `GET /identity/:network/owner/:address`.
///
/// There is no `balanceOf` on SVM, so ownership comes from a `getProgramAccounts`
/// scan filtered by the AgentAccount discriminator and the `owner` field.
async fn get_identity_by_owner_solana<A>(
    facilitator: A,
    network: crate::network::Network,
    address: &str,
) -> axum::response::Response
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    let owner = match solana_erc8004::parse_agent_id(address) {
        Ok(pk) => pk,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid Solana address: {}", address)
                })),
            )
                .into_response();
        }
    };

    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Solana(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("No Solana provider for {}", network) })),
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
                    "error": format!("No Solana ERC-8004 program IDs for {}", network)
                })),
            )
                .into_response();
        }
    };

    info!(network = %network, owner = %owner, "Resolving agent ID by owner (Solana)");

    match solana_erc8004::find_agents_by_owner(
        provider.rpc_client(),
        &owner,
        &programs.agent_registry,
    )
    .await
    {
        Ok(agents) if agents.is_empty() => {
            info!(network = %network, owner = %owner, "Owner holds no agent in registry");
            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": format!("Address {} does not own any agent on {}", owner, network),
                    "balance": "0"
                })),
            )
                .into_response()
        }
        Ok(agents) => {
            let balance = agents.len();
            let (_, agent) = &agents[0];
            let agent_id = solana_erc8004::bytes_to_pubkey(&agent.asset).to_string();
            info!(
                network = %network, agent_id = %agent_id, owner = %owner, balance,
                "Resolved agent by owner (Solana)"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "agentId": agent_id,
                    "owner": owner.to_string(),
                    "agentUri": agent.agent_uri,
                    "network": network.to_string(),
                    "balance": balance.to_string()
                })),
            )
                .into_response()
        }
        // Never a 404: callers persist "not registered" from a 404 and stop
        // asking, which is how a transient RPC failure became a permanent null
        // agent ID once already (INC-2026-07-21). Here it would also let a
        // caller re-mint an agent that already exists. 503 says retry.
        Err(e) => {
            warn!(network = %network, owner = %owner, error = %e, "Owner lookup inconclusive");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": format!("Could not determine agent ID for {} on {}: {}", owner, network, e),
                    "retryable": true
                })),
            )
                .into_response()
        }
    }
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
pub async fn post_register<A>(
    State(facilitator): State<A>,
    headers: HeaderMap,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider> + Sync,
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
        // A Solana recipient must be a base58 pubkey; an EVM address here is a
        // client bug that would otherwise burn a mint before failing.
        let solana_recipient = match &request.recipient {
            Some(addr) => match solana_erc8004::parse_agent_id(&addr.to_string()) {
                Ok(pk) => Some(pk),
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(RegisterAgentResponse {
                            success: false,
                            agent_id: None,
                            transaction: None,
                            transfer_transaction: None,
                            owner: None,
                            error: Some(format!(
                                "recipient must be a base58 Solana address on {}",
                                network
                            )),
                            network,
                        }),
                    )
                        .into_response();
                }
            },
            None => None,
        };

        let programs = match solana_erc8004::get_program_ids(&network) {
            Some(prog) => prog,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(RegisterAgentResponse {
                        success: false,
                        agent_id: None,
                        transaction: None,
                        transfer_transaction: None,
                        owner: None,
                        error: Some(format!("No Solana ERC-8004 programs for {}", network)),
                        network,
                    }),
                )
                    .into_response();
            }
        };

        // Resolve root_config -> collection -> registry_config from on-chain state
        let registry_ctx =
            match solana_erc8004::read_registry_context(p.rpc_client(), &programs.agent_registry)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    error!(network = %network, error = %e, "Failed to resolve registry context");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(RegisterAgentResponse {
                            success: false,
                            agent_id: None,
                            transaction: None,
                            transfer_transaction: None,
                            owner: None,
                            error: Some(format!("Failed to read registry config: {}", e)),
                            network,
                        }),
                    )
                        .into_response();
                }
            };

        // Generate a new keypair for the NFT asset
        let asset_keypair = solana_sdk::signature::Keypair::new();
        let asset_pubkey = asset_keypair.pubkey();
        let fee_payer = p.keypair();

        let ix = solana_erc8004::build_register_ix(
            &programs,
            &registry_ctx,
            &asset_pubkey,
            &fee_payer.pubkey(),
            &request.agent_uri,
        );

        // Register requires both fee_payer and asset keypairs to sign
        match solana_erc8004::send_erc8004_transaction_with_signers(
            p.rpc_client(),
            fee_payer,
            &[fee_payer, &asset_keypair],
            vec![ix],
        )
        .await
        {
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
                            &programs,
                            &asset_pubkey,
                            &fee_payer.pubkey(),
                            &entry.key,
                            entry.value.as_bytes(),
                            false,
                        );
                        if let Err(e) = solana_erc8004::send_erc8004_transaction(
                            p.rpc_client(),
                            fee_payer,
                            vec![ix],
                        )
                        .await
                        {
                            warn!(
                                key = %entry.key, error = %e,
                                "Failed to set metadata (agent registered successfully)"
                            );
                        }
                    }
                }

                // Initialize the ATOM stats account while the facilitator still owns
                // the agent. Only the owner may do this, so after a transfer it is
                // out of reach, and without it every feedback is recorded unscored.
                let ix = solana_erc8004::build_initialize_stats_ix(
                    &programs,
                    &registry_ctx.collection,
                    &asset_pubkey,
                    &fee_payer.pubkey(),
                );
                let atom_ready = match solana_erc8004::send_erc8004_transaction(
                    p.rpc_client(),
                    fee_payer,
                    vec![ix],
                )
                .await
                {
                    Ok(stats_sig) => {
                        info!(
                            network = %network, agent_id = %agent_id, tx = %stats_sig,
                            "ATOM stats initialized"
                        );
                        true
                    }
                    Err(e) => {
                        // Not fatal: the agent exists and is usable, it just cannot
                        // accumulate reputation until someone initializes the stats.
                        error!(
                            network = %network, agent_id = %agent_id, error = %e,
                            "Failed to initialize ATOM stats; feedback for this agent will not be scored"
                        );
                        false
                    }
                };

                // Hand the agent to the requested owner, last so the steps above still
                // run under facilitator ownership.
                let mut transfer_tx = None;
                let mut final_owner = MixedAddress::Solana(fee_payer.pubkey());
                if let Some(recipient) = solana_recipient {
                    let ix = solana_erc8004::build_transfer_agent_ix(
                        &programs,
                        &registry_ctx.collection,
                        &asset_pubkey,
                        &fee_payer.pubkey(),
                        &recipient,
                    );
                    match solana_erc8004::send_erc8004_transaction(
                        p.rpc_client(),
                        fee_payer,
                        vec![ix],
                    )
                    .await
                    {
                        Ok(xfer_sig) => {
                            info!(
                                network = %network, agent_id = %agent_id, tx = %xfer_sig,
                                recipient = %recipient, "Agent transferred to recipient"
                            );
                            transfer_tx =
                                Some(crate::types::TransactionHash::Solana(xfer_sig.into()));
                            final_owner = MixedAddress::Solana(recipient);
                        }
                        Err(e) => {
                            // The mint succeeded, so report the agent rather than lose
                            // it, but do not claim it was delivered.
                            error!(
                                network = %network, agent_id = %agent_id, error = %e,
                                "Agent minted but transfer to recipient failed"
                            );
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(RegisterAgentResponse {
                                    success: false,
                                    agent_id: Some(agent_id),
                                    transaction: Some(crate::types::TransactionHash::Solana(
                                        sig.into(),
                                    )),
                                    transfer_transaction: None,
                                    owner: Some(MixedAddress::Solana(fee_payer.pubkey())),
                                    error: Some(format!(
                                        "Agent minted but transfer to {} failed, it is still \
                                         held by the facilitator: {}",
                                        recipient, e
                                    )),
                                    network,
                                }),
                            )
                                .into_response();
                        }
                    }
                }

                if !atom_ready {
                    warn!(
                        network = %network, agent_id = %agent_id,
                        "Agent registered without ATOM stats"
                    );
                }

                return (
                    StatusCode::OK,
                    Json(RegisterAgentResponse {
                        success: true,
                        agent_id: Some(agent_id),
                        transaction: Some(crate::types::TransactionHash::Solana(sig.into())),
                        transfer_transaction: transfer_tx,
                        owner: Some(final_owner),
                        error: None,
                        network,
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                error!(network = %network, error = %e, "Solana registration failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(RegisterAgentResponse {
                        success: false,
                        agent_id: None,
                        transaction: None,
                        transfer_transaction: None,
                        owner: None,
                        error: Some(format!("Registration failed: {}", e)),
                        network,
                    }),
                )
                    .into_response();
            }
        }
    }

    // ── EVM registration: dispatch sync vs async (P1 pollable, P3 in-flight lock) ──
    let async_mode = wants_async(&headers);
    let key = register_jobs::inflight_key(&network, &request.agent_uri, &request.recipient);
    let job_id = match register_jobs::begin(network, key) {
        register_jobs::BeginOutcome::AlreadyInflight(job) => {
            return already_inflight_response(async_mode, job);
        }
        register_jobs::BeginOutcome::Started(id) => id,
    };

    if async_mode {
        // P1: respond immediately with a jobId; drive the mint on a background
        // task so the facilitator's on-chain latency leaves the caller's path.
        let fac = facilitator.clone();
        let jid = job_id.clone();
        let uri = request.agent_uri.clone();
        let meta = request.metadata.clone();
        let recip = request.recipient.clone();
        tokio::spawn(async move {
            let (_status, resp) =
                run_evm_registration(fac, network, uri, meta, recip, Some(jid.clone())).await;
            register_jobs::finalize_from_response(&jid, &resp);
        });
        return accepted_response(&job_id);
    }

    // Synchronous path (default): unchanged response contract, now covered by
    // the in-flight lock (P3) and the receipt-wait timeout (P2).
    let (status, resp) = run_evm_registration(
        facilitator,
        network,
        request.agent_uri.clone(),
        request.metadata.clone(),
        request.recipient.clone(),
        Some(job_id.clone()),
    )
    .await;
    register_jobs::finalize_from_response(&job_id, &resp);
    (status, Json(resp)).into_response()
}

/// EVM ERC-8004 registration core: mint (+ optional transfer to recipient).
/// Shared by the synchronous and async (`Prefer: respond-async`) paths of
/// `POST /register`. Applies the same `TX_RECEIPT_TIMEOUT_SECS` receipt-wait
/// bound as `/settle` (P2) and, when `job_id` is set, records progress in the
/// register job store so `GET /register/status/{job_id}` can report `agentId`.
async fn run_evm_registration<A>(
    facilitator: A,
    network: crate::network::Network,
    agent_uri: String,
    metadata: Option<Vec<MetadataEntryParam>>,
    recipient: Option<MixedAddress>,
    job_id: Option<String>,
) -> (StatusCode, RegisterAgentResponse)
where
    A: HasProviderMap + Send + Sync + 'static,
    A::Map: ProviderMap<Value = NetworkProvider> + Sync,
{
    let provider_map = facilitator.provider_map();
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("No ERC-8004 contracts for network {}", network)),
                    network,
                },
            );
        }
    };

    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            error!(network = %network, "No EVM provider available for network");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("No EVM provider available for network {}", network)),
                    network,
                },
            );
        }
    };

    // Create Identity Registry contract instance
    let identity_registry =
        IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone());

    // Resolve the facilitator's own wallet address up front. Both the
    // stranded-NFT recovery path (FAC-1 #2) and the post-mint transfer need it,
    // and resolving it before minting means a misconfigured (non-EVM) signer
    // fails fast without first spending gas on a mint. On an EVM provider the
    // signer is always EVM; the non-EVM arm is purely defensive.
    let facilitator_mixed = provider.signer_address();
    let facilitator_address = match &facilitator_mixed {
        MixedAddress::Evm(addr) => addr.0,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some("Unexpected non-EVM signer address".to_string()),
                    network,
                },
            );
        }
    };

    // The stranded-NFT recovery key for this registration (FAC-1 #2). It is
    // `Some` ONLY when a later retry could actually reclaim a stranded token —
    // i.e. an EVM recipient and a non-empty (trimmed) agentURI — so recording a
    // stranded NFT (on transfer failure) and recovering it (on retry) share this
    // exact precondition and can never drift: we never record what recovery
    // could not consume, and never orphan a record after a successful delivery.
    let recovery_key: Option<String> = match &recipient {
        Some(MixedAddress::Evm(_)) if !agent_uri.trim().is_empty() => {
            register_jobs::inflight_key(&network, &agent_uri, &recipient)
        }
        _ => None,
    };

    // ── Idempotency check: if recipient already owns an agent, return it ──
    // Uses ownerOf() iteration — works on ALL chains including SKALE which lacks
    // ERC-721 Enumerable and limits eth_getLogs to 2000 blocks.
    let check_owner = match &recipient {
        Some(MixedAddress::Evm(addr)) => Some(addr.0),
        _ => None,
    };
    if let Some(target_owner) = check_owner {
        if let Ok(balance) = identity_registry.balanceOf(target_owner).call().await {
            if balance > alloy::primitives::U256::ZERO {
                match resolve_first_token_by_owner(
                    provider.inner(),
                    network,
                    contracts.identity_registry,
                    target_owner,
                    balance,
                )
                .await
                {
                    Ok(Some(id)) => {
                        info!(
                            network = %network,
                            agent_id = id,
                            owner = %target_owner,
                            "Idempotent register: returning existing agent"
                        );
                        return (
                            StatusCode::OK,
                            RegisterAgentResponse {
                                success: true,
                                agent_id: Some(id.to_string()),
                                transaction: None,
                                transfer_transaction: None,
                                owner: Some(MixedAddress::Evm(crate::types::EvmAddress(
                                    target_owner,
                                ))),
                                error: None,
                                network,
                            },
                        );
                    }
                    // Unreachable on this path, and deliberately so. We only
                    // get here with `balance > 0`, and `resolve_first_token_by_owner`
                    // now reports a non-zero balance with nothing found as a
                    // CONTRADICTION (`Err`) rather than a clean miss. This arm
                    // used to be where a wrong scan range turned into a
                    // duplicate mint for someone who already had an identity:
                    // the balance said they owned an agent, the scan could not
                    // find it, and we minted anyway.
                    Ok(None) => {
                        warn!(
                            network = %network,
                            owner = %target_owner,
                            balance = %balance,
                            "Recipient has balance but no matching token; not minting a duplicate"
                        );
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            RegisterAgentResponse {
                                success: false,
                                agent_id: None,
                                transaction: None,
                                transfer_transaction: None,
                                owner: Some(MixedAddress::Evm(crate::types::EvmAddress(
                                    target_owner,
                                ))),
                                error: Some(format!(
                                    "Recipient {target_owner} holds {balance} agent(s) that the \
                                     registry scan could not attribute \
                                     (retryable, no mint attempted)"
                                )),
                                network,
                            },
                        );
                    }
                    // Inconclusive scan: we do NOT know whether this recipient
                    // already has an identity. Minting here is what produced
                    // duplicate agent NFTs on Base — each duplicate grows the
                    // registry that made the scan fail in the first place
                    // (INC-2026-07-06). Fail closed and let the caller retry.
                    Err(e) => {
                        warn!(
                            network = %network,
                            owner = %target_owner,
                            balance = %balance,
                            error = %e,
                            "Register aborted: could not determine whether recipient already owns an agent"
                        );
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            RegisterAgentResponse {
                                success: false,
                                agent_id: None,
                                transaction: None,
                                transfer_transaction: None,
                                owner: Some(MixedAddress::Evm(crate::types::EvmAddress(
                                    target_owner,
                                ))),
                                error: Some(format!(
                                    "Could not verify existing identity for {target_owner} \
                                     (retryable, no mint attempted): {e}"
                                )),
                                network,
                            },
                        );
                    }
                }
            }
        }
    }

    // ── FAC-1 #2: recover a stranded self-mint instead of minting anew ──
    // If a prior registration for this exact network|uri|recipient triple minted
    // the identity NFT but then failed to transfer it, the NFT is stranded in
    // the facilitator wallet. Rather than mint a fresh one (orphaning the
    // stranded token and returning a different agentId), reclaim it. This is
    // record-based and recipient-keyed: it can only ever hand back an NFT THIS
    // facilitator minted for THIS recipient+uri, so there is no on-chain URI
    // scan and thus no planted-token poisoning and no cross-recipient
    // mis-delivery. It costs zero extra RPC on the happy path — the on-chain
    // view calls run only when a stranded record actually exists for this key.
    if register_recovery_enabled() {
        if let (Some(key), Some(MixedAddress::Evm(ref recip))) = (&recovery_key, &recipient) {
            let recipient_address = recip.0;
            if let Some(stranded) = register_jobs::get_stranded(key) {
                info!(
                    network = %network,
                    agent_id = stranded.agent_id,
                    recipient = %recipient_address,
                    "Found a stranded agent NFT for this key; attempting recovery instead of minting"
                );
                match try_recover_stranded_nft(
                    provider,
                    contracts.identity_registry,
                    facilitator_address,
                    recipient_address,
                    stranded.agent_id,
                    &agent_uri,
                    network,
                )
                .await
                {
                    StrandedRecovery::Recovered(transfer_tx) => {
                        register_jobs::clear_stranded(key);
                        info!(
                            network = %network,
                            agent_id = stranded.agent_id,
                            recipient = %recipient_address,
                            "Recovered stranded agent NFT to recipient (no new mint)"
                        );
                        return (
                            StatusCode::OK,
                            RegisterAgentResponse {
                                success: true,
                                agent_id: Some(stranded.agent_id.to_string()),
                                transaction: stranded.mint_tx.clone(),
                                transfer_transaction: Some(transfer_tx),
                                owner: recipient.clone(),
                                error: None,
                                network,
                            },
                        );
                    }
                    StrandedRecovery::Gone => {
                        // The token is no longer facilitator-held (moved) or its
                        // on-chain URI does not match: the record is stale. Drop
                        // it and fall through to a fresh mint.
                        register_jobs::clear_stranded(key);
                    }
                    StrandedRecovery::Transient => {
                        // A transient RPC/transfer error: keep the record for a
                        // later retry, but still mint now so the caller gets an
                        // identity on this call. If that fresh mint then delivers
                        // successfully, the transfer path clears this record
                        // (FAC-1 #1) so it can't dangle past the delivery.
                    }
                }
            }
        }
    }

    // Build the registration call based on provided parameters
    let agent_uri = agent_uri.clone();
    let has_metadata = metadata.as_ref().map_or(false, |m| !m.is_empty());

    // Legacy chains (SKALE) need explicit gasPrice to avoid EIP-1559 rejection
    let legacy_gas_price = if !provider.is_eip1559() {
        match provider.inner().get_gas_price().await {
            Ok(gp) => Some(gp),
            Err(e) => {
                error!(error = %e, "Failed to get gas price for legacy chain");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    RegisterAgentResponse {
                        success: false,
                        agent_id: None,
                        transaction: None,
                        transfer_transaction: None,
                        owner: None,
                        error: Some(format!("Failed to get gas price: {}", e)),
                        network,
                    },
                );
            }
        }
    } else {
        None
    };

    let register_result = if has_metadata {
        // Convert metadata params to contract MetadataEntry structs
        let metadata_entries: Vec<MetadataEntry> = metadata
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
    } else if !agent_uri.is_empty() {
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
                RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("Failed to send registration transaction: {}", e)),
                    network,
                },
            );
        }
    };

    // Wait for receipt
    let receipt = match pending_tx
        .with_timeout(Some(evm_receipt_timeout(&network)))
        .get_receipt()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(network = %network, error = %e, "Failed to get registration receipt");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("Registration transaction failed: {}", e)),
                    network,
                },
            );
        }
    };

    let reg_tx_hash = receipt.transaction_hash;

    // Symmetric to the transfer path: a reverted-but-mined mint still returns a
    // receipt (status == 0). A reverted mint emits no Registered event, so the
    // totalSupply() fallback below would otherwise resolve an UNRELATED agentId
    // and could transfer the wrong NFT to the recipient — reject it outright.
    if !receipt.status() {
        error!(network = %network, tx = %reg_tx_hash, "Registration transaction reverted on-chain");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            RegisterAgentResponse {
                success: false,
                agent_id: None,
                transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                transfer_transaction: None,
                owner: None,
                error: Some("Registration transaction reverted on-chain".to_string()),
                network,
            },
        );
    }
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
                        RegisterAgentResponse {
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
                        },
                    );
                }
            }
        }
    };
    let agent_id_str = agent_id.to_string();

    if let Some(ref jid) = job_id {
        register_jobs::set_mint_confirmed(
            jid,
            crate::types::TransactionHash::Evm(reg_tx_hash.0),
            agent_id_str.clone(),
            facilitator_mixed.clone(),
        );
    }
    let mut final_owner = facilitator_mixed;
    let mut transfer_tx: Option<crate::types::TransactionHash> = None;

    // If recipient is specified, transfer the NFT
    if let Some(ref recipient) = recipient {
        let recipient_address = match recipient {
            MixedAddress::Evm(addr) => addr.0,
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    RegisterAgentResponse {
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
                    },
                );
            }
        };

        info!(
            agent_id = agent_id,
            from = %facilitator_address,
            to = %recipient_address,
            "Transferring agent NFT to recipient"
        );

        match transfer_agent_nft(
            provider,
            contracts.identity_registry,
            facilitator_address,
            recipient_address,
            agent_id,
            network,
        )
        .await
        {
            Ok(tx) => {
                info!(
                    network = %network,
                    tx = ?tx,
                    agent_id = agent_id,
                    recipient = %recipient_address,
                    "Agent NFT transferred successfully"
                );
                transfer_tx = Some(tx);
                final_owner = recipient.clone();
                // FAC-1 #1: a successful delivery clears any stranded record for
                // this key, so a transient-recovery-then-fresh-mint success can't
                // leave a dangling record pointing at an orphaned self-mint that
                // the idempotency short-circuit would never revisit.
                if let Some(k) = &recovery_key {
                    register_jobs::clear_stranded(k);
                }
            }
            Err(e) => {
                error!(error = %e, "Transfer failed - agent registered but NOT transferred");
                // FAC-1 #2: remember the stranded self-mint keyed by the exact
                // triple so a later retry for this recipient+uri reclaims THIS
                // token instead of minting a fresh one. `recovery_key` is `Some`
                // only when recovery could consume it (EVM recipient + non-empty
                // agentURI). Gated on the same kill-switch as the recovery read
                // so we never write a record recovery is disabled from consuming;
                // the ungated success-path clear still reaps any leftover record.
                if register_recovery_enabled() {
                    if let Some(k) = &recovery_key {
                        register_jobs::record_stranded(
                            k.clone(),
                            agent_id,
                            Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                        );
                    }
                }
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    RegisterAgentResponse {
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
                    },
                );
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
        RegisterAgentResponse {
            success: true,
            agent_id: Some(agent_id_str),
            transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
            transfer_transaction: transfer_tx,
            owner: Some(final_owner),
            error: None,
            network,
        },
    )
}

/// FAC-1 #2 kill-switch. Stranded-NFT recovery (reclaiming an agent NFT that a
/// prior registration minted but failed to transfer, instead of minting a fresh
/// one and orphaning the stranded token) is record-based and recipient-keyed, so
/// it is safe by construction and defaults ON. Set `ENABLE_REGISTER_RECOVERY` to
/// `false`/`0`/`no`/`off` to disable it.
fn register_recovery_enabled() -> bool {
    std::env::var("ENABLE_REGISTER_RECOVERY")
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

/// Outcome of attempting to recover a stranded agent NFT (FAC-1 #2).
enum StrandedRecovery {
    /// The stranded NFT was verified and transferred to the recipient; carries
    /// the transfer transaction hash.
    Recovered(crate::types::TransactionHash),
    /// The record is stale (token no longer facilitator-held, or its on-chain
    /// URI does not match this request): drop the record and mint fresh.
    Gone,
    /// A transient RPC/transfer error: keep the record and mint fresh this time.
    Transient,
}

/// Attempt to reclaim a stranded self-minted agent NFT and hand it to the
/// recipient, instead of minting a new one (FAC-1 #2).
///
/// Safety is by construction: the caller only reaches here with an `agent_id` it
/// recorded from a PRIOR mint-succeeded-transfer-failed attempt for this exact
/// recipient+uri. Before transferring we still re-verify on-chain that (a) the
/// facilitator wallet is the current owner (so we never move a token that has
/// left the wallet or that we do not own), and (b) the token's `tokenURI`
/// byte-exactly matches the requested `agentURI` (never case-folded — IPFS CIDs
/// and URL paths are case-sensitive), so a registry whose `tokenURI` is not the
/// raw `agentURI` simply degrades to minting.
async fn try_recover_stranded_nft(
    provider: &crate::chain::evm::EvmProvider,
    registry: alloy::primitives::Address,
    facilitator: alloy::primitives::Address,
    recipient: alloy::primitives::Address,
    agent_id: u64,
    agent_uri: &str,
    network: crate::network::Network,
) -> StrandedRecovery {
    let identity_registry = IIdentityRegistry::new(registry, provider.inner().clone());
    let id = alloy::primitives::U256::from(agent_id);

    // (a) The NFT must still be held by the facilitator (our own self-mint).
    match identity_registry.ownerOf(id).call().await {
        Ok(owner) if owner == facilitator => {}
        Ok(other) => {
            warn!(
                agent_id,
                current_owner = %other,
                "Stranded recovery: NFT is no longer facilitator-held; will mint fresh"
            );
            return StrandedRecovery::Gone;
        }
        Err(e) => {
            warn!(agent_id, error = %e, "Stranded recovery: ownerOf read failed; will mint fresh");
            return StrandedRecovery::Transient;
        }
    }

    // (b) tokenURI must byte-match this request's agentURI (trim surrounding
    //     whitespace only; never case-fold).
    match identity_registry.tokenURI(id).call().await {
        Ok(on_chain_uri) if on_chain_uri.trim() == agent_uri.trim() => {}
        Ok(on_chain_uri) => {
            warn!(
                agent_id,
                on_chain_uri = %on_chain_uri,
                "Stranded recovery: tokenURI does not match request URI; will mint fresh"
            );
            return StrandedRecovery::Gone;
        }
        Err(e) => {
            warn!(agent_id, error = %e, "Stranded recovery: tokenURI read failed; will mint fresh");
            return StrandedRecovery::Transient;
        }
    }

    // (c) Transfer the recovered NFT to the recipient (on-chain success checked).
    match transfer_agent_nft(
        provider,
        registry,
        facilitator,
        recipient,
        agent_id,
        network,
    )
    .await
    {
        Ok(tx) => StrandedRecovery::Recovered(tx),
        Err(e) => {
            warn!(agent_id, error = %e, "Stranded recovery: transfer failed; will mint fresh");
            StrandedRecovery::Transient
        }
    }
}

/// Send a `safeTransferFrom` of an ERC-8004 agent NFT from the facilitator to a
/// recipient, wait for the receipt (bounded by [`evm_receipt_timeout`]), and
/// REQUIRE that the transaction actually succeeded on-chain.
///
/// A reverted `safeTransferFrom` still yields a receipt, so `receipt.status()`
/// MUST be checked — otherwise a non-delivery (e.g. a recipient contract without
/// `onERC721Received`, or a race that reverts) would be reported to the caller as
/// a successful transfer. Shared by the normal post-mint transfer and the
/// stranded-NFT recovery path so this success check can never drift between them.
async fn transfer_agent_nft(
    provider: &crate::chain::evm::EvmProvider,
    registry: alloy::primitives::Address,
    from: alloy::primitives::Address,
    to: alloy::primitives::Address,
    agent_id: u64,
    network: crate::network::Network,
) -> Result<crate::types::TransactionHash, String> {
    let identity_registry = IIdentityRegistry::new(registry, provider.inner().clone());
    let transfer_call =
        identity_registry.safeTransferFrom(from, to, alloy::primitives::U256::from(agent_id));

    // Legacy chains (SKALE) need an explicit gasPrice to avoid EIP-1559 rejection.
    let send_result = if !provider.is_eip1559() {
        let gas_price = provider
            .inner()
            .get_gas_price()
            .await
            .map_err(|e| format!("Failed to get gas price for transfer: {e}"))?;
        transfer_call.gas_price(gas_price).send().await
    } else {
        transfer_call.send().await
    };

    let pending = send_result.map_err(|e| format!("Failed to send transfer transaction: {e}"))?;
    let receipt = pending
        .with_timeout(Some(evm_receipt_timeout(&network)))
        .get_receipt()
        .await
        .map_err(|e| format!("Transfer receipt failed: {e}"))?;

    if !receipt.status() {
        return Err(format!(
            "Transfer reverted on-chain (tx {})",
            receipt.transaction_hash
        ));
    }
    Ok(crate::types::TransactionHash::Evm(
        receipt.transaction_hash.0,
    ))
}

/// Whether the caller opted into async registration (P1). Honored via the
/// RFC 7240 `Prefer: respond-async` header, or the convenience `X-Async: true`.
fn wants_async(headers: &HeaderMap) -> bool {
    let prefer = headers
        .get("prefer")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("respond-async"))
        .unwrap_or(false);
    let x_async = headers
        .get("x-async")
        .and_then(|v| v.to_str().ok())
        .map(|s| matches!(s.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    prefer || x_async
}

/// Receipt-wait timeout for an ERC-8004 registration/transfer tx (P2). Mirrors
/// the `/settle` path in `chain::evm`: 30s default, longer for slow L1s,
/// overridable via `TX_RECEIPT_TIMEOUT_SECS`.
fn evm_receipt_timeout(network: &crate::network::Network) -> std::time::Duration {
    use crate::network::Network;
    let default_secs = match network {
        Network::Ethereum => 900,
        Network::Base => 90,
        _ => 30,
    };
    let secs = std::env::var("TX_RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default_secs);
    std::time::Duration::from_secs(secs)
}

/// Response when a registration for the same in-flight key is already running
/// (P3). Async callers get the existing job; sync callers get `409 Conflict`
/// instead of a double-mint that would revert on-chain.
fn already_inflight_response(async_mode: bool, job: register_jobs::RegisterJob) -> Response {
    if async_mode {
        let loc = format!("/register/status/{}", job.job_id);
        return (
            StatusCode::OK,
            [(axum::http::header::LOCATION, loc)],
            Json(job),
        )
            .into_response();
    }
    (
        StatusCode::CONFLICT,
        Json(RegisterAgentResponse {
            success: false,
            agent_id: job.agent_id.clone(),
            transaction: job.transaction.clone(),
            transfer_transaction: job.transfer_transaction.clone(),
            owner: job.owner.clone(),
            error: Some(
                "A registration for this agent is already in progress; retry later or poll \
                 GET /register/status/{jobId}"
                    .to_string(),
            ),
            network: job.network,
        }),
    )
        .into_response()
}

/// `202 Accepted` for an async registration: returns the freshly-created job
/// (status `pending`) plus a `Location` header pointing at the status endpoint.
fn accepted_response(job_id: &str) -> Response {
    let loc = format!("/register/status/{job_id}");
    let body = match register_jobs::get(job_id) {
        Some(job) => serde_json::to_value(&job)
            .unwrap_or_else(|_| json!({ "jobId": job_id, "status": "pending" })),
        None => json!({ "jobId": job_id, "status": "pending" }),
    };
    (
        StatusCode::ACCEPTED,
        [(axum::http::header::LOCATION, loc)],
        Json(body),
    )
        .into_response()
}

/// `GET /register/status/{job_id}`: poll an async ERC-8004 registration (P1).
/// Returns the job with `agentId` once the mint confirms, or `404` if the
/// job id is unknown or its result has aged out of the store.
#[instrument(skip_all, fields(job_id))]
pub async fn get_register_status(Path(job_id): Path<String>) -> impl IntoResponse {
    match register_jobs::get(&job_id) {
        Some(job) => (StatusCode::OK, Json(job)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "register job not found or expired",
                "jobId": job_id,
            })),
        )
            .into_response(),
    }
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

        let rpc = solana_provider.rpc_client();
        match solana_erc8004::read_metadata_entry(
            rpc,
            &asset_pubkey,
            &params.key,
            &programs.agent_registry,
        )
        .await
        {
            Ok(entry) => {
                let hex_value = format!("0x{}", hex::encode(&entry.metadata_value));
                let utf8_value = String::from_utf8(entry.metadata_value.clone()).ok();

                return (
                    StatusCode::OK,
                    Json(json!({
                        "agentId": params.agent_id,
                        "key": entry.metadata_key,
                        "value": hex_value,
                        "valueUtf8": utf8_value,
                        "immutable": entry.immutable != 0,
                        "network": network
                    })),
                )
                    .into_response();
            }
            Err(solana_erc8004::SolanaErc8004Error::AccountNotFound(_)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": format!("Metadata key '{}' not set for agent {} on {}", params.key, params.agent_id, network)
                    })),
                )
                    .into_response();
            }
            Err(e) => {
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

        // The registry keeps no agent counter on-chain; the Metaplex Core collection
        // referenced by RootConfig is the golden source.
        let rpc = solana_provider.rpc_client();
        let collection =
            match solana_erc8004::read_collection_pubkey(rpc, &programs.agent_registry).await {
                Ok(c) => c,
                Err(e) => {
                    error!(network = %network, error = %e, "Failed to read Solana root config");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "error": format!("Failed to query total supply: {}", e)
                        })),
                    )
                        .into_response();
                }
            };

        match solana_erc8004::read_collection_supply(rpc, &collection).await {
            Ok(supply) => {
                // current_size is net of burns, matching ERC-721 totalSupply semantics.
                let total = supply.current_size as u64;
                info!(
                    network = %network,
                    total_supply = total,
                    num_minted = supply.num_minted,
                    "Queried Solana registry total supply"
                );
                return (
                    StatusCode::OK,
                    Json(json!({
                        "totalSupply": total,
                        "numMinted": supply.num_minted,
                        "collection": collection.to_string(),
                        "network": network
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                error!(network = %network, error = %e, "Failed to query Solana collection supply");
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
                // This is the answer on EVERY deployed ERC-8004 registry, not an
                // edge case: verified on-chain 2026-09-01 across celo and base,
                // where `supportsInterface(ERC721Enumerable)` is false too. This
                // endpoint answering a dead 501 on all nine networks is what let
                // "totalSupply is already in the ABI and this endpoint already
                // uses it" pass review as a reason to make the owner lookup
                // depend on it -- true about the ABI, false about the chain, and
                // nobody called the endpoint to find out.
                //
                // So answer with the number the registry can actually produce,
                // labelled for what it is. `highestAgentId` is the top of the ID
                // range, which equals the supply only when nothing was burned;
                // the two are reported under different names on purpose.
                warn!(
                    network = %network,
                    error = %e,
                    "totalSupply() not available on this contract version; deriving the highest agent ID instead"
                );
                match discover_max_agent_id(provider.inner(), contracts.identity_registry).await {
                    Ok(highest) => {
                        store_registry_bound(network, contracts.identity_registry, highest);
                        (
                            StatusCode::OK,
                            Json(json!({
                                "totalSupply": serde_json::Value::Null,
                                "highestAgentId": highest,
                                "source": "ownerOf-probe",
                                "network": network,
                                "hint": "This registry does not implement ERC721Enumerable, so totalSupply() is unavailable. highestAgentId is the top of the agent-ID range, derived by probing ownerOf; it equals the supply only if no agent was ever burned."
                            })),
                        )
                            .into_response()
                    }
                    Err(probe_err) => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": format!("totalSupply() is unavailable and the ownerOf probe reached no verdict: {probe_err}"),
                            "retryable": true,
                            "network": network
                        })),
                    )
                        .into_response(),
                }
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

#[cfg(test)]
mod owner_scan_tests {
    use super::*;

    /// Real revert payloads observed on Base and Optimism mainnet (2026-07-24)
    /// for `ownerOf` on a nonexistent token. `0x7e273289` is the
    /// `ERC721NonexistentToken(uint256)` selector.
    #[test]
    fn classifies_contract_reverts_as_token_absent() {
        assert!(is_execution_revert(
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted", data: Some(RawValue("0x7e273289000000000000000000000000000000000000000000000000000000003b9ac9ff")) })"#
        ));
        assert!(is_execution_revert(
            r#"server returned an error response: error code 3: execution reverted"#
        ));
        assert!(is_execution_revert("ERC721: invalid token ID"));
    }

    /// These carry no verdict about the token. Treating any of them as
    /// "token absent" truncated the scan range and let `/register` mint
    /// duplicate agent NFTs (INC-2026-07-06).
    #[test]
    fn classifies_rpc_failures_as_inconclusive() {
        assert!(!is_execution_revert(
            "server returned an error response: error code -32003: out of gas: \
             gas exhausted during memory expansion: 600000000"
        ));
        assert!(!is_execution_revert(
            "server returned an error response: error code -32007: rate limit exceeded"
        ));
        assert!(!is_execution_revert("HTTP error 429 Too Many Requests"));
        // Non-standard provider error shape (observed from polygon-rpc.com).
        assert!(!is_execution_revert(
            "message: API key disabled, reason: tenant disabled, json-rpc code: -32051"
        ));
        assert!(!is_execution_revert("operation timed out"));
        assert!(!is_execution_revert("error sending request for url"));
    }

    /// An unrecognised error must default to inconclusive, never to
    /// "token absent" — the whole point of failing closed.
    #[test]
    fn unknown_errors_default_to_inconclusive() {
        assert!(!is_execution_revert("something nobody has seen before"));
        assert!(!is_execution_revert(""));
    }

    /// The batch cap must keep a whole-registry scan inside both limits that
    /// were measured against Base: the 600M gas cap the production RPC
    /// enforces, and the ~16.4k-call response-body cap of the public node.
    #[test]
    fn batch_size_stays_within_measured_rpc_limits() {
        const MEASURED_PUBLIC_NODE_CALL_CAP: u64 = 16_383;
        assert!(OWNER_SCAN_BATCH < MEASURED_PUBLIC_NODE_CALL_CAP);
        // The Base registry held ~58.4k agents when the scan broke (2026-07-24)
        // and 83,984 when it was measured again on 2026-09-01. Both must stay
        // reachable within the batch ceiling.
        for base_registry_size in [58_400u64, 83_984] {
            assert!(
                base_registry_size.div_ceil(OWNER_SCAN_BATCH) <= OWNER_SCAN_MAX_BATCHES,
                "a Base registry of {base_registry_size} no longer fits the scan"
            );
        }
    }

    /// A clean miss is 404 and carries NO `retryable` flag.
    ///
    /// Paired with the test below: these two outcomes must never collapse into
    /// each other. Callers persist a 404 as "not registered" and stop asking.
    #[test]
    fn owner_without_agent_answers_404_without_retryable() {
        let (status, Json(body)) = owner_lookup_response(
            crate::network::Network::Base,
            alloy::primitives::Address::ZERO,
            "1",
            Ok(None),
        );
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            body.get("retryable").is_none(),
            "a clean miss must not be advertised as retryable: {body}"
        );
    }

    /// An RPC failure is 503 + `retryable: true`, never 404.
    ///
    /// Answering 404 here turns a transient outage of OURS into a permanent
    /// wrong answer on the caller's side, and on the registration path mints a
    /// duplicate agent for someone who already has one (INC-2026-07-21).
    #[test]
    fn inconclusive_lookup_answers_503_and_retryable() {
        let (status, Json(body)) = owner_lookup_response(
            crate::network::Network::Base,
            alloy::primitives::Address::ZERO,
            "1",
            Err("Multicall3 batch 1..=630 failed: error code -32007: rate limit exceeded".into()),
        );
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body.get("retryable").and_then(|v| v.as_bool()),
            Some(true),
            "an inconclusive lookup must tell the caller to retry: {body}"
        );
    }

    /// The batch ceiling applies to the SPAN of a scan, not to its upper bound.
    ///
    /// The `totalSupply` fallback rescans only the tail the first pass could
    /// not see, so a tail high in a large registry must cost the batches of the
    /// tail rather than the batches of everything below it.
    #[test]
    fn range_batch_count_follows_the_span() {
        // A 630-agent registry scanned from 1: one batch.
        assert_eq!((630u64 - 1 + 1).div_ceil(OWNER_SCAN_BATCH), 1);
        // The tail of a 58.4k registry whose totalSupply read 58k: still one
        // batch, not the 30 that rescanning from 1 would have cost.
        assert_eq!((58_400u64 - 58_001 + 1).div_ceil(OWNER_SCAN_BATCH), 1);
    }

    /// The identity-read limit has to stay well clear of the sweep that
    /// motivated it (~21 req/min aggregated, measured 2026-08-29). A limit
    /// sized against imagined abuse instead of measured traffic is how every
    /// 429 in the last bazaar incident turned out to be a legitimate client.

    // ---------------------------------------------------------------------
    // Registry bound search
    //
    // These tests exist because the code they cover replaced a version whose
    // production path had NO coverage at all. The previous revision put
    // `totalSupply()` first and the sequential probe second; `totalSupply()`
    // reverts on every registry actually deployed, so the "fallback" was the
    // only branch that ever ran, and 1,264 green tests never touched it. The
    // search below is therefore driven end to end against an in-memory
    // registry, so the path production takes is the path the suite exercises.
    // ---------------------------------------------------------------------

    /// Run the pure halves of [`discover_max_agent_id`] against a registry
    /// whose agent IDs are exactly `1..=max_id`, and report both the answer and
    /// the number of Multicall3 round trips it cost.
    ///
    /// This is the whole search minus the RPC: `multicall_owner_of` is the only
    /// piece replaced, by the in-memory `id <= max_id`.
    fn simulate_bound_search(max_id: u64) -> Result<(u64, u32), String> {
        let ladder = bound_ladder_points();
        let present: Vec<bool> = ladder.iter().map(|id| *id <= max_id).collect();
        let (mut lo, mut hi) = match bracket_from_ladder(&ladder, &present)? {
            Some(bracket) => bracket,
            None => return Err("registry is empty".to_string()),
        };

        let mut round_trips: u32 = 1;
        while hi > lo + 1 {
            if round_trips >= BOUND_SEARCH_MAX_ROUNDS {
                return Err(format!("did not converge (bracket {lo}..{hi})"));
            }
            let points = refine_points(lo, hi, BOUND_SEARCH_PROBES_PER_ROUND);
            if points.is_empty() {
                break;
            }
            let present: Vec<bool> = points.iter().map(|id| *id <= max_id).collect();
            let (next_lo, next_hi) = narrow_bracket(lo, hi, &points, &present);
            round_trips += 1;
            if next_lo == lo && next_hi == hi {
                return Err(format!("stalled at {lo}..{hi}"));
            }
            lo = next_lo;
            hi = next_hi;
        }
        Ok((lo, round_trips))
    }

    /// The number of STRICTLY SEQUENTIAL `eth_call`s the exponential probe plus
    /// binary search spent on the same question, for the record.
    fn legacy_sequential_probe_cost(max_id: u64) -> u32 {
        let mut hi: u64 = 1;
        let mut calls: u32 = 0;
        loop {
            calls += 1;
            if hi > max_id {
                break;
            }
            hi = hi.saturating_mul(2);
        }
        let mut lo = hi / 2;
        while lo < hi.saturating_sub(1) {
            calls += 1;
            let mid = lo + (hi - lo) / 2;
            if mid <= max_id {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        calls
    }

    /// The search must land on the exact maximum for every registry size we
    /// have seen or can plausibly reach, and never cost more than four round
    /// trips doing it.
    ///
    /// Four is the number that matters: the sequential probe this replaced took
    /// ~28 round trips on the celo registry, in series, on EVERY cold lookup.
    #[test]
    fn bound_search_is_exact_and_bounded_at_four_round_trips() {
        // 630 = the Base registry when the batch cap was sized. 9,725 = the
        // celo agent whose lookup was measured at 11.12s in production on
        // 2026-09-01. 58,400 = Base when the scan broke. The rest are the
        // boundaries: the smallest registry, a power of two, one below a power
        // of two, and the ladder ceiling itself.
        for max_id in [
            1u64, 2, 3, 7, 8, 630, 1_024, 9_802, 16_383, 16_384, 58_400, 100_000, 1_000_000,
            16_777_215,
        ] {
            let (found, round_trips) = simulate_bound_search(max_id)
                .unwrap_or_else(|e| panic!("search failed for max_id={max_id}: {e}"));
            assert_eq!(found, max_id, "wrong bound for a registry of {max_id}");
            assert!(
                round_trips <= 4,
                "max_id={max_id} cost {round_trips} round trips; the search must stay within 4"
            );
        }
    }

    /// The measurement that motivated all of this, pinned so it cannot be
    /// quietly undone: the celo lookup went from ~28 sequential round trips to
    /// at most 4 parallel ones.
    #[test]
    fn the_celo_registry_stops_costing_twenty_eight_serial_round_trips() {
        const CELO_MAX_AGENT_ID: u64 = 9_802;
        let legacy = legacy_sequential_probe_cost(CELO_MAX_AGENT_ID);
        let (_, now) = simulate_bound_search(CELO_MAX_AGENT_ID).expect("search must converge");
        assert!(
            legacy >= 25,
            "the legacy probe cost {legacy} calls; this test is calibrated against ~28"
        );
        assert!(
            (now as u32) * 6 < legacy,
            "the new search costs {now} round trips against the legacy {legacy}; \
             that is not the order-of-magnitude win this replaced it for"
        );
    }

    /// A registry larger than the ladder can describe must be an ERROR.
    ///
    /// The sequential probe it replaces stopped doubling at 1,000,000 and then
    /// binary-searched inside the range it had already left, silently answering
    /// with a maximum far below the real one -- which turns every agent above
    /// that point into a 404, and a 404 is what callers persist as "not
    /// registered".
    #[test]
    fn a_registry_past_the_ladder_ceiling_errors_rather_than_truncating() {
        let points = bound_ladder_points();
        let all_present = vec![true; points.len()];
        let outcome = bracket_from_ladder(&points, &all_present);
        assert!(
            outcome.is_err(),
            "a registry beyond the ceiling must report no verdict, got {outcome:?}"
        );

        // The boundary, stated rather than implied: the highest ID the search
        // can resolve is one BELOW the ceiling. A registry whose maximum is the
        // ceiling itself is indistinguishable from one that runs past it, and
        // both are reported as no verdict rather than guessed at.
        let ceiling = 1u64 << BOUND_LADDER_MAX_EXP;
        assert!(simulate_bound_search(ceiling - 1).is_ok());
        assert!(simulate_bound_search(ceiling).is_err());

        // And that boundary has to stay far above anything real. The Base
        // registry is the largest we have, between 65,536 and 262,144 agents on
        // 2026-09-01 -- but the SCAN caps out first, so that is the number to
        // watch: OWNER_SCAN_MAX_BATCHES x OWNER_SCAN_BATCH agents.
        let scannable = OWNER_SCAN_MAX_BATCHES * OWNER_SCAN_BATCH;
        assert!(
            ceiling > scannable,
            "the bound search must be able to describe any registry the scan can walk"
        );
        // Measured on-chain 2026-09-01: the Base registry held 83,984 agents.
        // If this assertion ever fails, the fix is an owner index, not a bigger
        // cap -- see OWNER_SCAN_MAX_BATCHES.
        const BASE_REGISTRY_SIZE_2026_09_01: u64 = 83_984;
        assert!(
            scannable >= BASE_REGISTRY_SIZE_2026_09_01 * 2,
            "the scan reaches {scannable} agents against a Base registry already at              {BASE_REGISTRY_SIZE_2026_09_01}; that is less than 2x headroom"
        );
    }

    /// Nothing exists at all: a clean empty registry, not an error.
    #[test]
    fn an_empty_registry_brackets_to_none() {
        let points = bound_ladder_points();
        let none_present = vec![false; points.len()];
        assert_eq!(bracket_from_ladder(&points, &none_present), Ok(None));
    }

    /// A burned agent sitting exactly on a power of two must not cut the
    /// bracket short. Taking the HIGHEST present point rather than stopping at
    /// the first absent one is what makes that free.
    #[test]
    fn a_hole_on_a_power_of_two_does_not_shorten_the_bracket() {
        // Registry holds 1..=1000 except that 8 was burned.
        let points = bound_ladder_points();
        let present: Vec<bool> = points.iter().map(|id| *id <= 1000 && *id != 8).collect();
        let (lo, hi) = bracket_from_ladder(&points, &present)
            .expect("must reach a verdict")
            .expect("registry is not empty");
        assert_eq!(lo, 512, "the highest present ladder point is 512, not 4");
        assert_eq!(hi, 1024);
    }

    /// A mismatched result count is a decode failure, not a bracket.
    #[test]
    fn a_short_probe_response_is_an_error() {
        assert!(bracket_from_ladder(&[1, 2, 4], &[true, true]).is_err());
    }

    /// Probe points must stay strictly inside the bracket, or a round can
    /// "confirm" the endpoint it was given and make no progress.
    #[test]
    fn refine_points_stay_strictly_inside_the_bracket() {
        for (lo, hi) in [(1u64, 2u64), (8, 16), (512, 1024), (1, 16_777_216)] {
            for p in refine_points(lo, hi, BOUND_SEARCH_PROBES_PER_ROUND) {
                assert!(
                    p > lo && p < hi,
                    "point {p} escaped the bracket ({lo},{hi})"
                );
            }
        }
    }

    /// When the gap is small enough to probe exhaustively, do that: the round
    /// after it is then exact rather than merely narrower.
    #[test]
    fn a_small_gap_is_probed_exhaustively() {
        assert_eq!(refine_points(10, 15, 1_000), vec![11, 12, 13, 14]);
        // Nothing to probe between adjacent IDs.
        assert!(refine_points(10, 11, 1_000).is_empty());
    }

    /// An absent point BELOW the confirmed floor must not drag `hi` under `lo`
    /// and invert the bracket.
    #[test]
    fn narrow_bracket_ignores_holes_below_the_confirmed_floor() {
        // 200 exists; 50 is a hole well below it.
        let (lo, hi) = narrow_bracket(1, 1024, &[50, 200], &[false, true]);
        assert_eq!(lo, 200);
        assert!(hi > lo, "bracket inverted: {lo}..{hi}");
        assert_eq!(hi, 1024);
    }

    /// Every batch this module sends must fit the limits measured against the
    /// production RPCs -- the bound search reuses the scan's transport, so it
    /// inherits the same cap and must respect it.
    #[test]
    fn every_probe_batch_fits_the_measured_rpc_cap() {
        assert!(
            BOUND_SEARCH_PROBES_PER_ROUND <= OWNER_SCAN_BATCH,
            "a refinement round would be rejected by the node"
        );
        assert!(
            bound_ladder_points().len() as u64 <= OWNER_SCAN_BATCH,
            "the ladder would be rejected by the node"
        );
        // The ladder must reach past every registry we have measured.
        assert!(
            *bound_ladder_points().last().unwrap() > 58_400,
            "the ladder cannot describe the Base registry"
        );
    }

    /// `totalSupply()` must not become load-bearing again.
    ///
    /// It is in the ABI and it reverts on every ERC-8004 registry actually
    /// deployed (verified on-chain 2026-09-01 on celo and base, where
    /// `supportsInterface(ERC721Enumerable)` is false as well). A revision that
    /// made the owner lookup depend on it shipped as a complete no-op and held
    /// the facilitator's p99 at 11.4s. Re-verify with
    /// `scripts/erc8004_registry_capabilities.py` before changing this.
    #[test]
    fn the_bound_search_does_not_depend_on_total_supply() {
        // `include_str!` reads whatever line endings are on disk, and a Windows
        // checkout stores CRLF. Without this the `"\n}\n"` split below matches
        // nothing, `search` becomes the rest of the 113k-char file, and the
        // assertion fails on a `totalSupply()` that lives somewhere else
        // entirely -- a red test that says nothing about the function it names.
        // Same reason `lf()` exists in `agentic_surface_tests`.
        let src = include_str!("handlers.rs").replace("\r\n", "\n");
        // The body only: from the signature to the closing brace at column 0.
        let search = src
            .split("async fn discover_max_agent_id")
            .nth(1)
            .expect("discover_max_agent_id must exist")
            .split("\n}\n")
            .next()
            .expect("the function must have a body");
        assert!(
            !search.contains("totalSupply()."),
            "discover_max_agent_id calls totalSupply(), which reverts on every deployed registry"
        );
    }

    /// The scan batches must tile the requested span EXACTLY: no gap, no
    /// overlap, nothing outside it.
    ///
    /// A gap here does not fail loudly. It skips an agent, the scan reports no
    /// match, and the caller is told the address is not registered.
    #[test]
    fn scan_batches_tile_the_span_exactly() {
        for (first, last) in [
            (1u64, 1u64),
            (1, 630),
            (1, OWNER_SCAN_BATCH),
            (1, OWNER_SCAN_BATCH + 1),
            (1, 9_802),
            (1, 100_000),
            (58_001, 58_400),
            (OWNER_SCAN_BATCH, OWNER_SCAN_BATCH * 3),
        ] {
            let ranges = scan_batch_ranges(first, last);
            assert_eq!(
                ranges[0].0, first,
                "span {first}..={last} does not start at first"
            );
            assert_eq!(
                ranges[ranges.len() - 1].1,
                last,
                "span {first}..={last} does not end at last"
            );
            for pair in ranges.windows(2) {
                assert_eq!(
                    pair[1].0,
                    pair[0].1 + 1,
                    "span {first}..={last} has a gap or overlap at {pair:?}"
                );
            }
            for (s, e) in &ranges {
                assert!(s <= e, "inverted batch {s}..={e}");
                assert!(
                    e - s + 1 <= OWNER_SCAN_BATCH,
                    "batch {s}..={e} exceeds the measured RPC cap"
                );
            }
            let covered: u64 = ranges.iter().map(|(s, e)| e - s + 1).sum();
            assert_eq!(
                covered,
                last - first + 1,
                "span {first}..={last} miscounted"
            );
        }
    }

    /// An empty span produces no batches rather than one bogus one.
    #[test]
    fn an_inverted_span_produces_no_batches() {
        assert!(scan_batch_ranges(10, 9).is_empty());
    }

    /// Concurrency must change how long the scan takes, never what it answers.
    ///
    /// The wave size has to divide the work without ever letting a later batch
    /// be examined before an earlier one has been: `chunks` guarantees that,
    /// and this pins the property so a future rewrite to a single unbounded
    /// fan-out (which would return whichever batch answered first, not the
    /// lowest ID) fails here instead of in production.
    #[test]
    fn scan_waves_preserve_ascending_order() {
        assert!(OWNER_SCAN_WAVE >= 1, "a wave must issue at least one batch");
        let ranges = scan_batch_ranges(1, 100_000);
        let mut previous_end = 0u64;
        for wave in ranges.chunks(OWNER_SCAN_WAVE) {
            assert!(
                wave[0].0 > previous_end,
                "wave starting at {} overlaps the previous wave ending at {previous_end}",
                wave[0].0
            );
            previous_end = wave[wave.len() - 1].1;
        }
        assert_eq!(previous_end, 100_000);
        // A wave is a burst against the shared RPC budget; INC-2026-07-06 was
        // that budget running out.
        assert!(
            OWNER_SCAN_WAVE <= 8,
            "a wave of {OWNER_SCAN_WAVE} batches is too large a burst for the shared RPC budget"
        );
    }

    /// The scan order is decided by the balance, and only a balance of exactly
    /// one may free the order.
    ///
    /// With two or more tokens the contract of this lookup is the LOWEST ID, so
    /// batches must run low-to-high; returning whichever match turned up first
    /// would answer a different question. With exactly one token there is only
    /// one match in existence, so it is trivially the lowest whatever order
    /// found it.
    #[test]
    fn only_a_single_token_balance_frees_the_scan_order() {
        assert_eq!(
            ScanOrder::for_balance(alloy::primitives::U256::from(1)),
            ScanOrder::AnyMatch
        );
        for many in [2u64, 3, 17, 1_000] {
            assert_eq!(
                ScanOrder::for_balance(alloy::primitives::U256::from(many)),
                ScanOrder::LowestFirst,
                "a balance of {many} must still be scanned lowest-first"
            );
        }
        // Zero never reaches a scan, but if it ever did it must not take the
        // shortcut that assumes exactly one token exists.
        assert_eq!(
            ScanOrder::for_balance(alloy::primitives::U256::ZERO),
            ScanOrder::LowestFirst
        );
    }

    /// THE property. The batch order must always be a permutation: every batch
    /// exactly once, none invented, none dropped.
    ///
    /// A dropped index does not fail loudly -- it skips a slice of the registry,
    /// the scan reports no match, and the caller is told the address is not
    /// registered. Checked exhaustively across sizes, hint counts and hint
    /// positions, including hints outside the range and several hints landing in
    /// the same batch.
    #[test]
    fn the_batch_order_is_always_a_permutation() {
        for n in 1usize..=45 {
            let ranges = scan_batch_ranges(1, (n as u64) * OWNER_SCAN_BATCH);
            assert_eq!(ranges.len(), n);

            let mut hint_sets: Vec<Vec<u64>> = vec![Vec::new()];
            for (first, last) in &ranges {
                hint_sets.push(vec![*first]);
                hint_sets.push(vec![*last]);
                // Two hints in the SAME batch must collapse to one seed.
                hint_sets.push(vec![*first, *last]);
                // A hot pair spanning the registry, which is the Base shape.
                hint_sets.push(vec![*first, ranges[n - 1].1]);
            }
            // Out of range on both sides, and more hints than there are slots.
            hint_sets.push(vec![0]);
            hint_sets.push(vec![u64::MAX]);
            hint_sets.push(vec![0, u64::MAX]);
            hint_sets.push(vec![0, u64::MAX, 1, (n as u64) * OWNER_SCAN_BATCH + 1]);

            for hints in &hint_sets {
                let order = any_match_batch_order(&ranges, hints);
                let mut sorted = order.clone();
                sorted.sort_unstable();
                assert_eq!(
                    sorted,
                    (0..n).collect::<Vec<_>>(),
                    "n={n} hints={hints:?} produced {order:?}, which is not a permutation"
                );
            }
        }
    }

    /// The regression this version exists for: Base's traffic runs in TWO
    /// clusters, and both must land in the FIRST wave.
    ///
    /// Measured in production 2026-09-01 under 2.4.0: agents around 18,900
    /// (batch 10 of 42) and around 58,600 (batch 30). With a single hint the
    /// order ping-ponged -- ten of twelve lookups answered in 0.47-1.34s, and
    /// the two that followed a match from the other cluster took 4.1s and 5.2s.
    #[test]
    fn both_measured_base_clusters_land_in_the_first_wave() {
        let ranges = scan_batch_ranges(1, 83_984);
        assert_eq!(ranges.len(), 42, "the Base registry should be 42 batches");

        let order = any_match_batch_order(&ranges, &[18_897, 58_583]);
        for agent in [18_897u64, 58_583] {
            let position = order
                .iter()
                .position(|i| {
                    let (first, last) = ranges[*i];
                    agent >= first && agent <= last
                })
                .expect("every hint's batch must appear in the order");
            assert!(
                position < OWNER_SCAN_WAVE,
                "agent {agent} is examined at position {position}, outside the first wave"
            );
        }
    }

    /// A single hint cannot cover both clusters -- which is exactly why the
    /// cache holds several. Pins the defect so the cap cannot return to one.
    #[test]
    fn one_hint_alone_cannot_reach_the_other_cluster_in_time() {
        let ranges = scan_batch_ranges(1, 83_984);
        let order = any_match_batch_order(&ranges, &[58_583]);
        let position = order
            .iter()
            .position(|i| {
                let (first, last) = ranges[*i];
                18_897 >= first && 18_897 <= last
            })
            .unwrap();
        assert!(
            position >= OWNER_SCAN_WAVE,
            "this test is calibrated against a single hint missing the other cluster"
        );
        assert!(
            SCAN_HINT_SLOTS >= 2,
            "one remembered batch cannot serve traffic that runs in two clusters"
        );
    }

    /// Batches with no hint are reached by distance to the NEAREST hint, so a
    /// lookup landing between two clusters does not walk from an end.
    #[test]
    fn the_tail_fans_out_from_the_nearest_hint() {
        let ranges = scan_batch_ranges(1, 83_984);
        let order = any_match_batch_order(&ranges, &[18_897, 58_583]);
        let position_of = |agent: u64| {
            order
                .iter()
                .position(|i| {
                    let (first, last) = ranges[*i];
                    agent >= first && agent <= last
                })
                .unwrap()
        };
        assert!(position_of(20_500) < position_of(83_000));
        assert!(position_of(56_500) < position_of(2_500));
    }

    /// Without any hint, both ends must be reached before the middle: neither
    /// extreme may be the pathological case.
    #[test]
    fn without_a_hint_neither_end_is_pathological() {
        let ranges = scan_batch_ranges(1, 83_984);
        let order = any_match_batch_order(&ranges, &[]);
        let last_index = ranges.len() - 1;

        let top = order.iter().position(|i| *i == last_index).unwrap();
        let bottom = order.iter().position(|i| *i == 0).unwrap();
        assert!(top < OWNER_SCAN_WAVE, "the top batch waits {top} positions");
        assert!(
            bottom < OWNER_SCAN_WAVE,
            "the bottom batch waits {bottom} positions"
        );

        let middle = order.iter().position(|i| *i == last_index / 2).unwrap();
        assert!(middle > top && middle > bottom);
    }

    /// A hint from before the registry grew, or from outside the tail being
    /// rescanned, must clamp rather than panic or drop batches.
    #[test]
    fn a_hint_outside_the_range_clamps() {
        let tail = scan_batch_ranges(80_001, 83_984);
        for hints in [vec![1u64], vec![18_897], vec![u64::MAX], vec![1, u64::MAX]] {
            let order = any_match_batch_order(&tail, &hints);
            let mut sorted = order.clone();
            sorted.sort_unstable();
            assert_eq!(
                sorted,
                (0..tail.len()).collect::<Vec<_>>(),
                "hints={hints:?}"
            );
        }
    }

    /// The hints are per (network, registry), like the bound: the ERC-8004
    /// registries share one address across chains, so a Base hint must never
    /// steer a Celo scan.
    #[test]
    fn the_scan_hint_does_not_leak_across_networks() {
        let registry = alloy::primitives::Address::repeat_byte(0xEF);
        SCAN_HINT_CACHE.remove(&(crate::network::Network::Base, registry));
        SCAN_HINT_CACHE.remove(&(crate::network::Network::Celo, registry));

        store_scan_hint(crate::network::Network::Base, registry, 18_897);
        assert_eq!(
            scan_hints(crate::network::Network::Base, registry),
            vec![18_897]
        );
        assert!(
            scan_hints(crate::network::Network::Celo, registry).is_empty(),
            "Celo must not inherit Base's hints from the shared registry address"
        );

        SCAN_HINT_CACHE.remove(&(crate::network::Network::Base, registry));
    }

    /// The hints follow the traffic: most recent first, one per batch, capped.
    ///
    /// One per BATCH because two agents in the same batch are the same hint --
    /// probing it finds both -- and a cache full of one cluster is how the other
    /// one starves.
    #[test]
    fn the_scan_hints_follow_the_traffic() {
        let registry = alloy::primitives::Address::repeat_byte(0xBA);
        let network = crate::network::Network::Optimism;
        SCAN_HINT_CACHE.remove(&(network, registry));

        store_scan_hint(network, registry, 60_720);
        store_scan_hint(network, registry, 18_897);
        assert_eq!(
            scan_hints(network, registry)[0],
            18_897,
            "the latest match must lead -- traffic moves, the bound does not"
        );
        assert!(
            scan_hints(network, registry).contains(&60_720),
            "and the previous cluster must still be remembered, not replaced"
        );

        store_scan_hint(network, registry, 18_905);
        assert_eq!(
            scan_hints(network, registry),
            vec![18_905, 60_720],
            "two agents in one batch are one hint: probing it finds both"
        );

        for agent in [2_500u64, 6_500, 10_500, 14_500] {
            store_scan_hint(network, registry, agent);
        }
        let held = scan_hints(network, registry);
        assert_eq!(held.len(), SCAN_HINT_SLOTS);
        assert_eq!(held[0], 14_500, "the newest hint must lead");
        assert!(
            !held.contains(&60_720),
            "the oldest hint must fall off once the cap is reached"
        );

        SCAN_HINT_CACHE.remove(&(network, registry));
    }

    /// A zero balance with nothing found is a truthful "owns nothing".
    #[test]
    fn a_zero_balance_scan_that_finds_nothing_is_a_clean_miss() {
        assert_eq!(
            exhausted_scan_outcome(
                alloy::primitives::Address::ZERO,
                alloy::primitives::U256::ZERO,
                630,
            ),
            Ok(None)
        );
    }

    /// A NON-zero balance with nothing found is a contradiction, and must never
    /// be reported as "owns nothing".
    ///
    /// This is the arm that mattered: the registry says the address holds a
    /// token, the scan could not attribute one, so the RANGE was wrong.
    /// Answering `Ok(None)` here is a 404 on `GET /identity/../owner/..` --
    /// which callers persist as "not registered" and stop asking
    /// (INC-2026-07-21) -- and on `POST /register` it is read as permission to
    /// mint, handing a duplicate identity to somebody who already has one.
    #[test]
    fn a_balance_the_scan_cannot_explain_is_never_reported_as_owning_nothing() {
        let outcome = exhausted_scan_outcome(
            alloy::primitives::Address::ZERO,
            alloy::primitives::U256::from(1),
            630,
        );
        assert!(
            outcome.is_err(),
            "a non-zero balance with no match must reach no verdict, got {outcome:?}"
        );

        // And it must land on 503 + retryable, never 404.
        let (status, Json(body)) = owner_lookup_response(
            crate::network::Network::Base,
            alloy::primitives::Address::ZERO,
            "1",
            outcome.map(|_| None),
        );
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.get("retryable").and_then(|v| v.as_bool()), Some(true));
    }

    /// The registry bound is cached per (network, registry), and a later,
    /// LOWER reading must not walk back a higher one: the maximum only grows,
    /// so a lower value is a slower request finishing late, not news.
    #[test]
    fn a_stale_lower_bound_never_overwrites_a_higher_one() {
        let registry = alloy::primitives::Address::repeat_byte(0xAB);
        let network = crate::network::Network::Avalanche;
        REGISTRY_BOUND_CACHE.remove(&(network, registry));

        store_registry_bound(network, registry, 9_725);
        assert_eq!(cached_registry_bound(network, registry), Some(9_725));

        store_registry_bound(network, registry, 42);
        assert_eq!(
            cached_registry_bound(network, registry),
            Some(9_725),
            "a late, lower reading must not lower the cached bound"
        );

        store_registry_bound(network, registry, 10_000);
        assert_eq!(cached_registry_bound(network, registry), Some(10_000));

        REGISTRY_BOUND_CACHE.remove(&(network, registry));
    }

    /// The bound cache must be keyed by network as well as registry: the
    /// ERC-8004 registries share one deterministic address across every chain,
    /// so keying on the address alone would serve Base's bound for a Celo
    /// lookup -- the same trap [`OWNER_LOOKUP_CACHE`] documents.
    #[test]
    fn the_bound_cache_does_not_leak_across_networks() {
        let registry = alloy::primitives::Address::repeat_byte(0xCD);
        REGISTRY_BOUND_CACHE.remove(&(crate::network::Network::Base, registry));
        REGISTRY_BOUND_CACHE.remove(&(crate::network::Network::Celo, registry));

        store_registry_bound(crate::network::Network::Base, registry, 58_400);
        assert_eq!(
            cached_registry_bound(crate::network::Network::Celo, registry),
            None,
            "Celo must not inherit Base's bound from the shared registry address"
        );

        REGISTRY_BOUND_CACHE.remove(&(crate::network::Network::Base, registry));
    }

    #[test]
    fn identity_read_limit_leaves_headroom_over_measured_traffic() {
        let (per_ms, burst) = identity_read_rate_limit();
        let sustained_per_min = 60_000 / per_ms;
        assert!(
            sustained_per_min >= 100,
            "sustained {sustained_per_min}/min is too tight for a single-IP integrator"
        );
        assert!(
            burst >= 30,
            "burst {burst} is too small to absorb a fan-out"
        );
    }
}

#[cfg(test)]
mod discovery_query_tests {
    use super::*;

    /// The papercut this rejection exists for: `q` filters server-side, but
    /// `search` used to be accepted and ignored, so the caller got the whole
    /// unfiltered page back and could not tell.
    #[test]
    fn rejects_the_parameter_that_used_to_be_ignored() {
        assert_eq!(unknown_discovery_params("limit=3&search=logs"), ["search"]);
        assert_eq!(discovery_param_hint("search"), Some("q"));
    }

    #[test]
    fn accepts_every_documented_parameter() {
        let raw = "limit=10&offset=0&category=finance&network=eip155:8453\
                   &provider=tenjin&tag=market-data&source=self_registered\
                   &sourceFacilitator=ultravioleta&health=alive&tier=vip&q=logs";
        assert!(unknown_discovery_params(raw).is_empty());
    }

    #[test]
    fn accepts_an_empty_query_string() {
        assert!(unknown_discovery_params("").is_empty());
    }

    /// Parameter names are case-sensitive on the wire; only the hint lookup
    /// is case-insensitive.
    #[test]
    fn rejects_a_miscased_parameter_but_still_points_at_the_right_one() {
        assert_eq!(unknown_discovery_params("Limit=3"), ["Limit"]);
        assert_eq!(discovery_param_hint("Search"), Some("q"));
        assert_eq!(
            discovery_param_hint("source_facilitator"),
            Some("sourceFacilitator")
        );
    }

    #[test]
    fn reports_each_rejected_parameter_once_in_order() {
        assert_eq!(
            unknown_discovery_params("search=a&limit=1&page=2&search=b"),
            ["search", "page"]
        );
    }

    #[test]
    fn decodes_percent_encoded_parameter_names() {
        // `%71` is `q`, which is valid and must not be rejected.
        assert!(unknown_discovery_params("%71=logs").is_empty());
    }

    #[test]
    fn a_valueless_parameter_is_still_a_parameter() {
        assert_eq!(unknown_discovery_params("search"), ["search"]);
    }

    #[test]
    fn hints_only_when_a_single_replacement_is_obvious() {
        assert_eq!(discovery_param_hint("q"), None);
        assert_eq!(discovery_param_hint("wat"), None);
        assert_eq!(discovery_param_hint("page"), Some("offset"));
        assert_eq!(discovery_param_hint("per_page"), Some("limit"));
        assert_eq!(discovery_param_hint("status"), Some("health"));
        assert_eq!(discovery_param_hint("curation"), Some("tier"));
    }

    /// The names come from an unauthenticated caller and land in a response
    /// body, so both the count and each name are bounded.
    #[test]
    fn caps_what_is_echoed_back() {
        let many: Vec<String> = (0..20).map(|i| format!("bogus{i}")).collect();
        let response = unknown_params_response(&many);
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let long = vec!["x".repeat(500)];
        let capped: String = long[0].chars().take(MAX_REPORTED_PARAM_LEN).collect();
        assert_eq!(capped.len(), MAX_REPORTED_PARAM_LEN);
        assert_eq!(
            unknown_params_response(&long).status(),
            StatusCode::BAD_REQUEST
        );
    }

    /// A caller reading `supported` should be able to send every name back
    /// unchanged and be accepted.
    #[test]
    fn the_advertised_parameter_list_is_self_consistent() {
        let raw = DISCOVERY_QUERY_PARAMS
            .iter()
            .map(|name| format!("{name}=x"))
            .collect::<Vec<_>>()
            .join("&");
        assert!(unknown_discovery_params(&raw).is_empty());
    }
}

#[cfg(test)]
mod discovery_handler_tests {
    use super::*;

    fn default_params() -> DiscoveryQueryParams {
        DiscoveryQueryParams {
            limit: 10,
            offset: 0,
            category: None,
            network: None,
            provider: None,
            tag: None,
            source: None,
            source_facilitator: None,
            health: None,
            tier: None,
            q: None,
        }
    }

    async fn list(raw_query: &str) -> (StatusCode, serde_json::Value) {
        let registry = Arc::new(DiscoveryRegistry::new());
        let response = get_discovery_resources(
            State(registry),
            RawQuery(Some(raw_query.to_string())),
            Query(default_params()),
        )
        .await
        .into_response();

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("response body");
        let body = serde_json::from_slice(&bytes).expect("json body");
        (status, body)
    }

    /// The exact request that used to come back as an unfiltered page.
    #[tokio::test]
    async fn search_is_rejected_and_points_at_q() {
        let (status, body) = list("limit=3&search=logs").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "unknown query parameter: search");
        assert_eq!(body["hint"], "did you mean q?");
        assert!(body["supported"]
            .as_array()
            .expect("supported list")
            .contains(&json!("q")));
    }

    #[tokio::test]
    async fn a_supported_query_still_lists() {
        let (status, body) = list("limit=10&offset=0&q=logs&health=alive").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body["items"].is_array());
        assert_eq!(body["pagination"]["total"], 0);
    }

    #[tokio::test]
    async fn an_empty_query_string_still_lists() {
        let (status, body) = list("").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body["items"].is_array());
    }

    /// No hint when the intent is not obvious, but still a 400 rather than
    /// silence.
    #[tokio::test]
    async fn an_unrecognizable_parameter_is_rejected_without_a_hint() {
        let (status, body) = list("wat=1").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "unknown query parameter: wat");
        assert!(body["hint"].is_null());
    }

    #[tokio::test]
    async fn several_rejected_parameters_are_listed_together() {
        let (status, body) = list("search=a&page=2").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "unknown query parameters: search, page");
        // Ambiguous: two rejects with two different replacements, no hint.
        assert!(body["hint"].is_null());
    }
}

#[cfg(test)]
mod writer_lease_gate_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Serialises the tests that flip the process-global writer flag.
    ///
    /// Without this they pass under CI's `--test-threads=1` and fail on a plain
    /// `cargo test` — a test that is green only under a specific flag is a trap
    /// for whoever runs the suite next.
    ///
    /// `pub(super)` so the ERC-8004 admin-gate tests can take the same lock: they
    /// flip the same global to assert which of the two layers runs first.
    pub(super) static WRITER_FLAG: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build the same middleware over a trivial route.
    ///
    /// The handler is a stub on purpose: what is under test is the GATE and the
    /// fact that it is wired, not what the ERC-8004 handlers do afterwards.
    fn gated_router() -> Router {
        Router::new()
            .route("/write", post(|| async { "wrote" }))
            .layer(axum::middleware::from_fn(require_writer_lease))
    }

    async fn status_of(router: Router) -> StatusCode {
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/write")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    /// The writer passes through untouched — the gate must not cost availability
    /// on the instance that is supposed to be writing.
    #[tokio::test]
    async fn writer_is_allowed_through() {
        let _guard = WRITER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        crate::writer_lease::set_writer_for_test(true);
        assert_eq!(status_of(gated_router()).await, StatusCode::OK);
    }

    /// A non-writer that has nowhere to forward is shed with 503, not 500: the
    /// caller should retry rather than treat the request as malformed.
    ///
    /// This is now the FALLBACK, not the normal path. It is reached only when
    /// the holder's address is unknown — which is also the state on a
    /// single-task service, where nothing is lost because that task is the
    /// writer anyway.
    #[tokio::test]
    async fn non_writer_without_a_known_holder_is_shed_with_503() {
        let _guard = WRITER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        crate::writer_lease::set_writer_for_test(false);
        crate::writer_lease::set_holder_endpoint_for_test(None);
        let status = status_of(gated_router()).await;
        // Restored before the assert so a failure cannot leave the process
        // wedged as a non-writer for every later test.
        crate::writer_lease::set_writer_for_test(true);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// The loop guard, and the reason forwarding is safe to enable.
    ///
    /// A request that already carries [`FORWARDED_HEADER`] was sent here BY a
    /// peer that believed this task held the lease. If it does not, the only
    /// safe answer is 503. Forwarding it onward — to an address that may point
    /// back at the sender — is how two tasks bounce one request between them
    /// until it times out, turning a 503 into a hung connection.
    ///
    /// Note this asserts a 503 while a holder address IS set, so the test fails
    /// if the guard is ever removed: without it this request would be proxied
    /// instead of refused.
    #[tokio::test]
    async fn a_forwarded_request_is_never_forwarded_again() {
        let _guard = WRITER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        crate::writer_lease::set_writer_for_test(false);
        // A reachable-looking peer: the guard must win over it.
        crate::writer_lease::set_holder_endpoint_for_test(Some("http://10.0.0.9:8080"));

        let response = gated_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/write")
                    .header(crate::writer_lease::FORWARDED_HEADER, "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();

        crate::writer_lease::set_writer_for_test(true);
        crate::writer_lease::set_holder_endpoint_for_test(None);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// The regression that review caught and this suite did not.
    ///
    /// The hop budgeted 60s + 30s — a number that appears nowhere in the receipt
    /// path. The holder's wait is chosen PER NETWORK: Ethereum 900s, Base 90s,
    /// everything else 30s. So a forwarded Ethereum settle aborted at 90s with
    /// `forward_failed` while the transaction was still alive, and on Base the
    /// margin was negative once signing time counted. Both carry real traffic.
    ///
    /// Asserting against the SAME numbers the receipt path uses is the point: if
    /// somebody raises Ethereum's wait, this fails instead of silently
    /// reintroducing a hop that gives up before the holder does.
    #[test]
    fn forward_timeout_outlasts_every_per_network_receipt_wait() {
        // Mirrors `evm_receipt_timeout` and its twin in `chain::evm`.
        const PER_NETWORK_WAITS: [(&str, u64); 3] =
            [("ethereum", 900), ("base", 90), ("everything else", 30)];

        std::env::remove_var("TX_RECEIPT_TIMEOUT_SECS");
        let hop = writer_forward_timeout();

        for (network, wait) in PER_NETWORK_WAITS {
            assert!(
                hop > std::time::Duration::from_secs(wait),
                "the hop ({hop:?}) gives up before {network} finishes waiting {wait}s for a \
                 receipt, so a payment that lands would be reported as a failure",
            );
        }

        // The margin must be real, not a tie: a tie loses, because the holder
        // also spends time signing before its receipt wait even starts.
        assert!(hop >= std::time::Duration::from_secs(LONGEST_RECEIPT_WAIT_SECS + 30));
    }

    /// An explicit `TX_RECEIPT_TIMEOUT_SECS` replaces the per-network default
    /// everywhere, so the hop needs only that value plus the margin — budgeting
    /// the 900s worst case anyway would hold a connection open for a quarter of
    /// an hour against a wait the operator deliberately shortened.
    #[test]
    fn an_explicit_receipt_timeout_governs_the_hop() {
        std::env::set_var("TX_RECEIPT_TIMEOUT_SECS", "45");
        let hop = writer_forward_timeout();
        std::env::remove_var("TX_RECEIPT_TIMEOUT_SECS");
        assert_eq!(hop, std::time::Duration::from_secs(75));

        // Garbage must not silently shorten the hop back to a tie with Ethereum.
        std::env::set_var("TX_RECEIPT_TIMEOUT_SECS", "not-a-number");
        let hop = writer_forward_timeout();
        std::env::remove_var("TX_RECEIPT_TIMEOUT_SECS");
        assert!(hop > std::time::Duration::from_secs(900));
    }

    /// With the kill-switch off, a non-writer refuses even when it knows where
    /// the holder is — the documented way back to the old behaviour.
    #[tokio::test]
    async fn forwarding_kill_switch_restores_refusal() {
        let _guard = WRITER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        crate::writer_lease::set_writer_for_test(false);
        crate::writer_lease::set_holder_endpoint_for_test(Some("http://10.0.0.9:8080"));
        std::env::set_var("ENABLE_WRITER_FORWARD", "false");

        let status = status_of(gated_router()).await;

        std::env::remove_var("ENABLE_WRITER_FORWARD");
        crate::writer_lease::set_writer_for_test(true);
        crate::writer_lease::set_holder_endpoint_for_test(None);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A forward that cannot connect degrades to the 503 it replaced, rather
    /// than surfacing a transport error or hanging. Port 1 on loopback is
    /// closed, so this exercises the real failure path, not a mock of it.
    #[tokio::test]
    async fn an_unreachable_holder_degrades_to_503() {
        let _guard = WRITER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        crate::writer_lease::set_writer_for_test(false);
        crate::writer_lease::set_holder_endpoint_for_test(Some("http://127.0.0.1:1"));

        let status = status_of(gated_router()).await;

        crate::writer_lease::set_writer_for_test(true);
        crate::writer_lease::set_holder_endpoint_for_test(None);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// EVM settles are the ones that need the single signer. Both protocol
    /// spellings must resolve, on either half of the request.
    #[test]
    fn evm_settle_bodies_are_routed_to_the_writer() {
        for body in [
            br#"{"paymentPayload":{"network":"base"}}"#.as_slice(),
            br#"{"paymentPayload":{"network":"eip155:8453"}}"#.as_slice(),
            br#"{"paymentRequirements":{"network":"arbitrum"}}"#.as_slice(),
            br#"{"network":"ethereum"}"#.as_slice(),
        ] {
            assert!(
                settle_body_targets_evm(body),
                "expected EVM routing for {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    /// The capacity half of the fix. Solana, Stellar, NEAR, Algorand, Sui and
    /// XRPL settle from their own signers and never touch the EVM nonce, so
    /// forwarding them would funnel six chain families through one task for no
    /// reason — trading a correctness bug for a capacity one.
    #[test]
    fn non_evm_settle_bodies_are_served_locally() {
        for body in [
            br#"{"paymentPayload":{"network":"solana"}}"#.as_slice(),
            br#"{"paymentPayload":{"network":"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"}}"#
                .as_slice(),
            br#"{"paymentPayload":{"network":"stellar"}}"#.as_slice(),
            br#"{"paymentRequirements":{"network":"near"}}"#.as_slice(),
        ] {
            assert!(
                !settle_body_targets_evm(body),
                "expected local handling for {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    /// The bias, stated as a test. An unreadable or unfamiliar body is treated
    /// as EVM and forwarded, because the holder can serve every family while a
    /// non-holder cannot serve EVM. Guessing "not EVM" here would resurrect the
    /// 503 for exactly the requests we understand least.
    #[test]
    fn an_unreadable_settle_body_is_routed_to_the_writer() {
        for body in [
            b"not json at all".as_slice(),
            br#"{}"#.as_slice(),
            br#"{"paymentPayload":{"network":"chain-we-have-never-heard-of"}}"#.as_slice(),
            br#"{"paymentPayload":{"network":12345}}"#.as_slice(),
            b"".as_slice(),
        ] {
            assert!(
                settle_body_targets_evm(body),
                "expected EVM routing for {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    // NOTE on what is NOT covered here: that the gate is actually ATTACHED to
    // every ERC-8004 route. Asserting it needs a `Router<FacilitatorLocal>` with
    // real state, and `ProviderCache::from_env()` is async and reads the
    // environment — too heavy and too environment-dependent for a unit test, and
    // a stub implementing `Facilitator + HasProviderMap` would be more mock than
    // test. Rather than fake it, the layer is applied INSIDE
    // `erc8004_write_routes()` itself so wiring is one reviewable line that
    // travels with the routes, instead of a call-site detail in main.rs that a
    // future caller can quietly drop.
}

/// The admin gate on `POST /feedback/revoke`.
///
/// What is under test is the GATE and its position in the stack, not what the
/// revoke handler does afterwards — reaching the real handler needs a provider
/// map and an RPC, and this route must be closed long before any of that.
#[cfg(test)]
mod erc8004_admin_gate_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Serialises the tests that mutate `ERC8004_ADMIN_TOKEN`, which is
    /// process-global. Same reasoning as `WRITER_FLAG`.
    static ADMIN_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const GOOD_TOKEN: &str = "test-erc8004-admin-token";

    fn gated_router() -> Router {
        Router::new()
            .route("/feedback/revoke", post(|| async { "revoked" }))
            .layer(axum::middleware::from_fn(require_erc8004_admin))
    }

    /// Both layers in the same order `erc8004_write_routes()` applies them.
    fn lease_and_admin_router() -> Router {
        Router::new()
            .route("/feedback/revoke", post(|| async { "revoked" }))
            .layer(axum::middleware::from_fn(require_writer_lease))
            .layer(axum::middleware::from_fn(require_erc8004_admin))
    }

    async fn status_with(router: Router, bearer: Option<&str>) -> StatusCode {
        let mut builder = Request::builder().method("POST").uri("/feedback/revoke");
        if let Some(token) = bearer {
            builder = builder.header(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {}", token),
            );
        }
        router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// Fail-closed. With no token configured the route is indistinguishable from
    /// one that does not exist — including for a caller who guesses a token.
    #[tokio::test]
    async fn without_a_configured_token_the_route_answers_404() {
        let _guard = ADMIN_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ERC8004_ADMIN_TOKEN_VAR);

        assert_eq!(
            status_with(gated_router(), None).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_with(gated_router(), Some(GOOD_TOKEN)).await,
            StatusCode::NOT_FOUND
        );
    }

    /// An empty value is not a configured token: setting the var to "" must not
    /// turn the surface on with a credential that a missing header also matches.
    #[tokio::test]
    async fn an_empty_token_does_not_open_the_route() {
        let _guard = ADMIN_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ERC8004_ADMIN_TOKEN_VAR, "");

        let status = status_with(gated_router(), None).await;
        std::env::remove_var(ERC8004_ADMIN_TOKEN_VAR);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Configured, but the caller does not present the right credential.
    ///
    /// 401 rather than 404 here is deliberate and matches the bazaar admin
    /// routes: once the operator has switched the surface on, a wrong token is a
    /// rejected request, not a missing route.
    #[tokio::test]
    async fn a_missing_or_wrong_token_is_401() {
        let _guard = ADMIN_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ERC8004_ADMIN_TOKEN_VAR, GOOD_TOKEN);

        let no_header = status_with(gated_router(), None).await;
        let wrong = status_with(gated_router(), Some("not-the-token")).await;
        // A prefix of the real token must not pass: the comparison is
        // constant-time over equal lengths, never a `starts_with`.
        let prefix = status_with(gated_router(), Some(&GOOD_TOKEN[..8])).await;
        std::env::remove_var(ERC8004_ADMIN_TOKEN_VAR);

        assert_eq!(no_header, StatusCode::UNAUTHORIZED);
        assert_eq!(wrong, StatusCode::UNAUTHORIZED);
        assert_eq!(prefix, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_configured_token_reaches_the_handler() {
        let _guard = ADMIN_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ERC8004_ADMIN_TOKEN_VAR, GOOD_TOKEN);

        let status = status_with(gated_router(), Some(GOOD_TOKEN)).await;
        std::env::remove_var(ERC8004_ADMIN_TOKEN_VAR);
        assert_eq!(status, StatusCode::OK);
    }

    /// The revoke gate must NOT reuse `BAZAAR_ADMIN_TOKEN`: different blast
    /// radii, different credential. Asserted rather than commented, because the
    /// tempting refactor is to collapse the two into one constant.
    #[tokio::test]
    async fn the_bazaar_token_does_not_unlock_the_revoke() {
        let _guard = ADMIN_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ERC8004_ADMIN_TOKEN_VAR);
        std::env::set_var(BAZAAR_ADMIN_TOKEN_VAR, "bazaar-only-token");

        let status = status_with(gated_router(), Some("bazaar-only-token")).await;
        std::env::remove_var(BAZAAR_ADMIN_TOKEN_VAR);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Authentication runs BEFORE the writer lease.
    ///
    /// If the order flipped, an unauthenticated probe against a task that does
    /// not hold the lease would get 503 — telling an anonymous caller that the
    /// route is live while the admin surface is supposed to look absent.
    #[tokio::test]
    async fn auth_runs_before_the_writer_lease() {
        let _env = ADMIN_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let _flag = super::writer_lease_gate_tests::WRITER_FLAG
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ERC8004_ADMIN_TOKEN_VAR);
        crate::writer_lease::set_writer_for_test(false);

        let status = status_with(lease_and_admin_router(), None).await;
        // Restored before the assert so a failure cannot leave the process
        // wedged as a non-writer for every later test.
        crate::writer_lease::set_writer_for_test(true);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

// ============================================================================
// Historical transactions and aggregated stats
// ============================================================================

/// Routes backed by the transaction store.
///
/// Their own router with its own state, like `discovery_routes` — the generic
/// `Facilitator` state stays untouched.
pub fn transaction_routes() -> Router<Arc<dyn crate::transaction_store::TransactionStore>> {
    Router::new()
        .route("/transactions", get(get_transactions))
        .route("/api/stats", get(get_stats))
        .route("/api/stats/history", get(get_stats_history))
}

#[derive(Debug, Deserialize)]
pub struct TransactionsQuery {
    limit: Option<usize>,
    network: Option<String>,
}

/// `GET /transactions` — recent operations, newest first.
///
/// Deliberately capped: this reads a live table, and an unbounded `limit` from
/// an unauthenticated caller is a way to turn a page load into a large bill.
pub async fn get_transactions(
    State(store): State<Arc<dyn crate::transaction_store::TransactionStore>>,
    Query(q): Query<TransactionsQuery>,
) -> impl IntoResponse {
    const MAX_LIMIT: usize = 200;
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_LIMIT);

    match store.recent(limit, q.network.as_deref()).await {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({
                "transactions": items,
                "count": items.len(),
                // Said in the payload, not just the docs: someone will diff this
                // against the chain and needs to know which way the gap points.
                "source": "facilitator records",
                "caveat": "Index of what the facilitator recorded, not a ledger. \
            Recording is best-effort and happens after settlement, so the chain is authoritative.",
            })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "transaction query failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "transaction store unavailable"})),
            )
                .into_response()
        }
    }
}

/// `GET /api/stats` — pre-aggregated totals, per network and asset.
///
/// Reads only the aggregate partition, so the cost is flat no matter how many
/// transactions have accumulated. Scanning the records instead would grow more
/// expensive every day the facilitator stays up.
/// `GET /api/stats/history`: settlement history reconstructed from the chain.
///
/// Deliberately NOT folded into `/api/stats`. That endpoint reports what this
/// facilitator observed and recorded; this one reports what was reconstructed
/// from on-chain evidence after the fact. Both are true and they are not the
/// same claim:
///
///   * the live figures know which endpoint was paid, because the x402 request
///     said so — but they only start when recording was switched on, and on
///     2026-08-03 that covered under 3% of the service's life;
///   * the reconstruction covers everything back to the first settlement in
///     October 2025, but it is silent on `resource` and `description`, which
///     never existed on-chain and cannot be recovered.
///
/// Serving them from one endpoint would let a consumer add a measured number to
/// a reconstructed one without noticing. Every row here carries `source`.
#[instrument(skip_all)]
pub async fn get_stats_history(
    State(store): State<Arc<dyn crate::transaction_store::TransactionStore>>,
) -> impl IntoResponse {
    let rows = match store.backfill().await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "history query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "history unavailable"})),
            )
                .into_response();
        }
    };

    // Rows carrying an asset moved money; rows carrying an op_kind are work the
    // facilitator performed and paid gas for. Summing them into one figure would
    // be the mistake this split exists to prevent.
    let settle_rows: Vec<_> = rows.iter().filter(|r| r.op_kind.is_none()).collect();
    let op_rows: Vec<_> = rows.iter().filter(|r| r.op_kind.is_some()).collect();

    let settles: u64 = settle_rows.iter().map(|r| r.count).sum();
    let volume: u128 = settle_rows.iter().map(|r| r.volume_atomic).sum();
    let operations: u64 = op_rows.iter().map(|r| r.count).sum();
    let networks: std::collections::BTreeSet<&str> =
        rows.iter().map(|r| r.network.as_str()).collect();
    let first = rows.iter().map(|r| r.first_ts).filter(|t| *t > 0).min();
    let last = rows.iter().map(|r| r.last_ts).max();

    let mut by_kind: std::collections::BTreeMap<&str, u64> = Default::default();
    for r in &op_rows {
        *by_kind
            .entry(r.op_kind.as_deref().unwrap_or("unknown"))
            .or_default() += r.count;
    }

    (
        StatusCode::OK,
        Json(json!({
            "source": "onchain-backfill",
            "note": "Reconstructed from on-chain evidence, not observed live. \
                     Amounts are exact; `resource` and `description` are absent \
                     because they never existed on-chain. Do NOT add these totals \
                     to /api/stats — that endpoint counts the same operations it \
                     recorded itself, and the two overlap.",
            "totals": {
                "settles": settles,
                "volumeAtomic": volume.to_string(),
                "operations": operations,
                "networks": networks.len(),
                "firstTs": first,
                "lastTs": last,
            },
            "operationsByKind": by_kind,
            "settlements": settle_rows.iter().map(|r| json!({
                "network": r.network,
                "asset": r.asset,
                "scheme": r.scheme.clone().unwrap_or_else(|| "exact".into()),
                "settles": r.count,
                "volumeAtomic": r.volume_atomic.to_string(),
                "decimals": r.asset.as_deref().and_then(|a| {
                    r.network.parse::<crate::network::Network>().ok()
                        .and_then(|n| crate::network::decimals_for_asset(n, a))
                }),
                "firstTs": r.first_ts,
                "lastTs": r.last_ts,
            })).collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

pub async fn get_stats(
    State(store): State<Arc<dyn crate::transaction_store::TransactionStore>>,
) -> impl IntoResponse {
    let aggregates = match store.aggregates().await {
        Ok(a) => a,
        Err(e) => {
            error!(error = %e, "stats query failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "stats unavailable"})),
            )
                .into_response();
        }
    };

    let settles_ok: u64 = aggregates.iter().map(|a| a.settles_ok).sum();
    let settles_failed: u64 = aggregates.iter().map(|a| a.settles_failed).sum();
    let verifies: u64 = aggregates.iter().map(|a| a.verifies).sum();
    let networks: std::collections::BTreeSet<&str> =
        aggregates.iter().map(|a| a.network.as_str()).collect();

    (
        StatusCode::OK,
        Json(json!({
            "totals": {
                "settlesOk": settles_ok,
                "settlesFailed": settles_failed,
                "verifies": verifies,
                "networks": networks.len(),
            },
            "byNetworkAndAsset": aggregates.iter().map(|a| json!({
                "network": a.network,
                "asset": a.asset,
                "settlesOk": a.settles_ok,
                "settlesFailed": a.settles_failed,
                "verifies": a.verifies,
                // A string: these are u256-shaped and a JSON number silently
                // loses precision past 2^53.
                "volumeAtomic": a.volume_atomic.to_string(),
                // Resolved per DEPLOYMENT, and served here so no consumer has to
                // join against /supported and guess. USDC is 6 nearly everywhere
                // and 18 on BSC; a client scaling by a constant is wrong there by
                // 10^12, and wrong in the direction that looks impressive rather
                // than broken. null means "we do not know this asset" -- render
                // the atomic value rather than inventing a scale.
                "decimals": a.network.parse::<crate::network::Network>().ok()
                    .and_then(|n| crate::network::decimals_for_asset(n, &a.asset)),
                "lastTs": a.last_ts,
            })).collect::<Vec<_>>(),
            "source": "facilitator records",
            "since": "Counting began when the transaction store was enabled; \
        operations before that are not included and are not zero.",
            "caveat": "Operations that ERROR are not recorded at all, so a 100% \
        success rate here means 'no failures were recorded', not 'no failures occurred'.",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod failure_category_tests {
    use super::failure_category;

    /// Classification keys on the variant NAME, which survives a reworded
    /// message. Keying on the message text would silently degrade every
    /// category to "other" the first time someone improved an error string.
    #[test]
    fn classifies_by_variant_not_by_message() {
        assert_eq!(
            failure_category(r#"InvalidSignature(0xabc, "recovered 0xdef, expected 0xabc")"#),
            "invalid_signature"
        );
        assert_eq!(
            failure_category(r#"InvalidSignature(0xabc, "totally different wording")"#),
            "invalid_signature"
        );
    }

    /// The reason this function exists. A ContractCall error wraps the raw
    /// transport error, which has carried an RPC URL with an API key in it.
    #[test]
    fn never_returns_anything_derived_from_the_payload() {
        let leaky = r#"ContractCall("TransportError(https://rpc.example/v1/SECRET_KEY_HERE)")"#;
        let category = failure_category(leaky);
        assert_eq!(category, "contract_revert");
        assert!(!category.contains("SECRET"), "the key must not survive");
        assert!(!category.contains("http"), "no URL may survive");
    }

    /// An unrecognised variant degrades to "other" rather than echoing itself.
    /// A future error type must not be able to leak by simply being new.
    #[test]
    fn unknown_variants_become_other() {
        assert_eq!(failure_category("SomeFutureError(0xdeadbeef)"), "other");
        assert_eq!(failure_category(""), "other");
    }

    /// Every category is a closed-set literal with no address or URL shape.
    #[test]
    fn every_category_is_safe_to_broadcast() {
        for debug in [
            "ContractCall(x)",
            "InvalidSignature(x)",
            "InsufficientFunds(x)",
            "InvalidTiming(x)",
            "BlockedAddress(x)",
            "UnsupportedNetwork(x)",
            "DecodingError(x)",
            "ClockError(x)",
            "Whatever(x)",
        ] {
            let c = failure_category(debug);
            assert!(!c.contains("0x") && !c.contains("http") && !c.contains('('));
        }
    }
}

#[cfg(test)]
mod canonical_network_name_tests {
    use super::canonical_network_name;

    /// The bug this guards: `/api/stats` keys rows on this string, and the
    /// alternate schemes take the network from the request, where the same
    /// chain legitimately arrives under three different spellings. Left alone
    /// they became three rows and a "networks with activity" count that
    /// overstated reality — seen in production the first time an escrow verify
    /// was recorded, as `base` AND `eip155:8453`.
    #[test]
    fn every_spelling_of_one_chain_collapses_to_one_name() {
        let base = canonical_network_name("base");
        assert_eq!(canonical_network_name("eip155:8453"), base);
        assert_eq!(canonical_network_name("base"), base);
    }

    #[test]
    fn caip2_is_resolved_for_more_than_one_family() {
        assert_eq!(canonical_network_name("eip155:1"), "ethereum");
        assert_eq!(canonical_network_name("eip155:137"), "polygon");
    }

    /// Inbound-only aliases are accepted by `FromStr` but must never be stored:
    /// `Display` emits one name and the index has to agree with it.
    #[test]
    fn inbound_alias_is_stored_under_the_emitted_name() {
        let canonical = canonical_network_name("skale-base");
        assert_eq!(canonical_network_name("skale"), canonical);
    }

    /// A chain we cannot name still happened. Passing it through under an odd
    /// label beats dropping the row and reporting a quieter, wrong total.
    #[test]
    fn unknown_network_survives_instead_of_vanishing() {
        assert_eq!(
            canonical_network_name("eip155:999999999"),
            "eip155:999999999"
        );
        assert_eq!(canonical_network_name("unknown"), "unknown");
    }
}

#[cfg(test)]
mod alt_request_fields_tests {
    use super::alt_request_fields;
    use serde_json::json;

    /// The shape that cost 84 of 317 settles their asset and amount.
    ///
    /// The top-level escrow envelope nests everything under a bare `payload`,
    /// which the extractor never looked inside. The rows still counted as
    /// operations, so `/api/stats` reported them as settles with volume 0 —
    /// indistinguishable from a settle that genuinely moved nothing.
    #[test]
    fn top_level_escrow_envelope_yields_asset_and_amount() {
        let body = json!({
            "scheme": "escrow",
            "payload": {
                "authorization": { "from": "0xaaa", "to": "0xbbb", "value": "30000" },
                "paymentInfo": {
                    "token": "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
                    "maxAmount": "30000",
                    "receiver": "0xccc"
                }
            }
        });
        let f = alt_request_fields(&body);
        assert_eq!(
            f.asset.as_deref(),
            Some("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
            "asset lives at payload.paymentInfo.token"
        );
        assert_eq!(f.amount.as_deref(), Some("30000"));
        assert_eq!(f.pay_to.as_deref(), Some("0xccc"));
    }

    /// The shapes that already worked must keep working — this fix widens the
    /// search, it does not move it.
    #[test]
    fn v1_requirements_still_resolve() {
        let body = json!({
            "paymentRequirements": {
                "network": "base",
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "maxAmountRequired": "10000",
                "payTo": "0xddd",
                "resource": "https://example.test/thing"
            }
        });
        let f = alt_request_fields(&body);
        assert_eq!(f.network.as_deref(), Some("base"));
        assert_eq!(f.amount.as_deref(), Some("10000"));
        assert_eq!(f.resource.as_deref(), Some("https://example.test/thing"));
    }

    #[test]
    fn v2_accepted_still_resolves() {
        let body = json!({
            "paymentPayload": {
                "accepted": { "network": "eip155:8453", "asset": "0xabc", "amount": "500" }
            }
        });
        let f = alt_request_fields(&body);
        assert_eq!(f.network.as_deref(), Some("eip155:8453"));
        assert_eq!(f.amount.as_deref(), Some("500"));
    }

    /// Absent stays absent. A guessed asset would be written into the index
    /// where nothing downstream could tell it from a measured one.
    #[test]
    fn nothing_is_invented_when_the_envelope_is_empty() {
        let f = alt_request_fields(&json!({ "scheme": "escrow" }));
        assert!(f.asset.is_none() && f.amount.is_none() && f.network.is_none());
    }
}

#[cfg(test)]
mod upstream_rpc_failure_tests {
    use super::is_upstream_rpc_failure;

    /// Real error strings captured from production while Celo's RPC was down.
    /// Every one of these returned 400 to the caller, which reads as "your
    /// request is wrong" for a failure the caller cannot influence.
    #[test]
    fn node_level_failures_are_recognised() {
        for e in [
            r#"ContractCall("ErrorResp(ErrorPayload { code: -32000, message: \"header not found\" })")"#,
            r#"error code -32000: historical state fa81e909 is not available"#,
            r#"ErrorResp(ErrorPayload { code: -32801, message: "no historical RPC is available for this historical (pre-L2) execution request" })"#,
            r#"ContractCall("ErrorResp(ErrorPayload { code: -32603, message: \"json: unsupported value\" })")"#,
            r#"Transport(Custom("Max retries exceeded server returned an error response"))"#,
        ] {
            assert!(is_upstream_rpc_failure(e), "should be upstream: {e}");
        }
    }

    /// These the chain DID execute and reject. The caller can act on them —
    /// fix the signature, fund the wallet — so 400 remains the honest answer.
    #[test]
    fn execution_reverts_stay_client_errors() {
        for e in [
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted: FiatTokenV2: invalid signature" })"#,
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted: ERC20: transfer amount exceeds balance" })"#,
        ] {
            assert!(!is_upstream_rpc_failure(e), "should stay client error: {e}");
        }
    }

    /// A revert wrapped in a transport error is still a revert: the chain
    /// answered. Without this precedence a bad signature would be reported as
    /// our outage, which is the same mistake in the opposite direction.
    #[test]
    fn revert_wins_over_a_nested_transport_code() {
        let e = r#"Transport(Custom("... code: -32000 ... execution reverted: FiatTokenV2: invalid signature"))"#;
        assert!(!is_upstream_rpc_failure(e));
    }

    /// Unrecognised text keeps the old behaviour. Guessing 502 would tell a
    /// caller with a genuinely broken payload to sit and wait for us.
    #[test]
    fn unknown_errors_are_not_promoted_to_upstream() {
        assert!(!is_upstream_rpc_failure("SchemeMismatch"));
        assert!(!is_upstream_rpc_failure("something entirely new"));
    }

    /// `txpool is full` never entered the mempool -- retrying is the correct
    /// caller behaviour (paired with `evm.rs` releasing the nonce on the same
    /// condition, so the retry lands on a clean slot instead of widening a
    /// gap).
    #[test]
    fn mempool_full_is_retryable() {
        assert!(is_upstream_rpc_failure(
            r#"ErrorResp(ErrorPayload { code: -32003, message: "txpool is full" })"#
        ));
    }

    /// `-32003` is overloaded: `eth_call`'s out-of-gas rejection carries the
    /// same code but is a real answer about the request, not an outage. This
    /// must stay a 400, not follow `-32003` into retryable.
    #[test]
    fn out_of_gas_32003_stays_a_client_error() {
        assert!(!is_upstream_rpc_failure(
            "server returned an error response: error code -32003: out of gas: \
             gas exhausted during memory expansion: 600000000"
        ));
    }
}

/// Exercises `impl IntoResponse for FacilitatorLocalError`'s `ContractCall`
/// arm directly (not just the `is_upstream_rpc_failure` classifier) — this is
/// the response the >95% plain-`/settle` EIP-3009 traffic actually gets,
/// wired to `is_upstream_rpc_failure` on 2026-08-28.
#[cfg(test)]
mod contract_call_response_tests {
    use super::*;

    /// `AuthCaptureEscrow.AfterAuthorizationExpiry(uint48,uint48)` — selector
    /// `0x36f2d211`, confirmed present in the compiled
    /// `contracts/out/AuthCaptureEscrow.sol/AuthCaptureEscrow.json` — was 173
    /// of 226 reverts on 2026-08-19/20 (a capture attempted after the
    /// authorization window: genuinely the client's fault, not ours).
    ///
    /// That specific revert is escrow-only and in production is classified by
    /// `OperatorError` at the escrow branch (`:2870`) / `/escrow/state`
    /// (`:3516`), not by this arm — plain `/settle` never calls
    /// `AuthCaptureEscrow`. This fixture is shaped like it anyway (real
    /// selector, schematic ABI-encoded args) to prove the point requested:
    /// the classifier is selector-agnostic. `is_upstream_rpc_failure` returns
    /// false for ANY string containing "execution reverted" before it looks
    /// at a single node code, so wiring it into this arm cannot turn an
    /// expired/invalid payload into an infinite retry loop — not for this
    /// selector, not for one we have never seen.
    #[test]
    fn a_genuine_contract_revert_stays_400_even_with_a_custom_error_selector() {
        let err = FacilitatorLocalError::ContractCall(
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted", data: Some(RawValue("0x36f2d211000000000000000000000000000000000000000000000000000000006901a2b400000000000000000000000000000000000000000000000000000000068ffab12")) })"#
                .to_string(),
        );
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// The two FiatTokenV2 (real USDC contract) reverts already proven in
    /// `upstream_rpc_failure_tests::execution_reverts_stay_client_errors` —
    /// re-asserted here against the actual response arm, since that is what
    /// plain `/settle` calls on every mainnet USDC transfer.
    #[test]
    fn a_real_usdc_revert_stays_400() {
        let err = FacilitatorLocalError::ContractCall(
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted: FiatTokenV2: invalid signature" })"#
                .to_string(),
        );
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    /// The failure mode this change targets: a node that cannot answer now
    /// gets 502 + `Retry-After` instead of the old unconditional 400 that
    /// told Execution Market "your request is wrong" during Celo's outage.
    #[test]
    fn a_node_failure_is_now_retryable_on_plain_settle() {
        let err = FacilitatorLocalError::ContractCall(
            r#"ErrorResp(ErrorPayload { code: -32000, message: "header not found" })"#.to_string(),
        );
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "30"
        );
    }

    /// `txpool is full` on the plain `/settle` path: retryable, same as the
    /// escrow branch, now that fix #1 releases the nonce on this exact
    /// condition so the retry lands on a clean slot.
    #[test]
    fn mempool_full_is_retryable_on_plain_settle() {
        let err = FacilitatorLocalError::ContractCall(
            r#"ErrorResp(ErrorPayload { code: -32003, message: "txpool is full" })"#.to_string(),
        );
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "30"
        );
    }
}

/// The agentic-discovery surfaces.
///
/// WHAT THESE TESTS ARE ACTUALLY DEFENDING
///     A scanner grades these files on three things at once: the status code,
///     the `content-type`, and the body being something other than the landing
///     page. Two of the three can be wrong while the endpoint looks perfectly
///     healthy in a browser -- a `skill.md` served as `text/html` renders fine
///     and scores zero. So every route is asserted on all three, and the
///     assertion lives here rather than in a runbook that nobody reruns.
#[cfg(test)]
mod agentic_surface_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    /// Every route in [`agentic_routes`] and the content type it must answer
    /// with. The list is literal on purpose: axum's `Router` does not expose its
    /// own paths, so the honest options are a hand-kept table that fails loudly
    /// or no check at all. Adding a route without adding a row here is caught by
    /// `the_table_covers_every_route`.
    const SURFACES: &[(&str, &str)] = &[
        ("/llms.txt", "text/plain"),
        ("/llms-full.txt", "text/plain"),
        ("/robots.txt", "text/plain"),
        ("/sitemap.xml", "application/xml"),
        ("/index.md", "text/markdown"),
        ("/skill.md", "text/markdown"),
        ("/auth.md", "text/markdown"),
        ("/workflows.json", "application/json"),
        ("/.well-known/agent-card.json", "application/json"),
        ("/.well-known/agent.json", "application/json"),
        ("/.well-known/x402", "application/json"),
        ("/.well-known/api-catalog", "application/linkset+json"),
        ("/.well-known/oauth-protected-resource", "application/json"),
        ("/.well-known/agent-skills/index.json", "application/json"),
        ("/.well-known/mcp/server-card.json", "application/json"),
        ("/.well-known/ard.json", "application/json"),
    ];

    async fn fetch(path: &str) -> (StatusCode, String, String) {
        let response = agentic_routes()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let ctype = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, ctype, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// 200, the right type, and a body with something in it.
    #[tokio::test]
    async fn every_surface_answers_200_with_its_declared_content_type() {
        for (path, expected_type) in SURFACES {
            let (status, ctype, body) = fetch(path).await;
            assert_eq!(status, StatusCode::OK, "{path} did not answer 200");
            assert!(
                ctype.starts_with(expected_type),
                "{path} answered content-type {ctype:?}, expected {expected_type:?}"
            );
            assert!(!body.trim().is_empty(), "{path} answered an empty body");
        }
    }

    /// None of them is HTML, and none of them is the landing page.
    ///
    /// This is the check that a scanner actually runs (`distinto_de_raiz`): a
    /// host that answers every unknown path with its SPA shell passes a naive
    /// status-code probe while publishing nothing. This facilitator does not do
    /// that today -- unknown paths 404 -- and this test is what keeps it true if
    /// a catch-all is ever added.
    #[tokio::test]
    async fn no_surface_is_html_or_the_landing_page() {
        let landing = include_str!("../static/index.html");
        for (path, _) in SURFACES {
            let (_, ctype, body) = fetch(path).await;
            assert!(
                !ctype.starts_with("text/html"),
                "{path} is served as HTML, which scores zero however correct the body is"
            );
            assert!(
                !body.trim_start().starts_with("<!DOCTYPE"),
                "{path} answered an HTML document"
            );
            assert_ne!(body, landing, "{path} answered the landing page");
        }
    }

    /// Adding a route without adding its row to [`SURFACES`] leaves it untested.
    ///
    /// Counted rather than introspected because axum will not enumerate its own
    /// paths; the number is the one thing a new `.route(...)` cannot silently
    /// keep true.
    #[test]
    fn the_table_covers_every_route() {
        let declared = include_str!("handlers.rs")
            .split("pub fn agentic_routes() -> Router {")
            .nth(1)
            .expect("agentic_routes must exist")
            .split("\n}")
            .next()
            .expect("agentic_routes must be closed")
            .matches(".route(")
            .count();
        assert_eq!(
            declared,
            SURFACES.len(),
            "agentic_routes declares {declared} routes but SURFACES lists {}; \
             a route with no row here is a route with no test",
            SURFACES.len()
        );
    }

    /// The JSON surfaces parse, and carry the fields a consumer indexes on.
    ///
    /// Not decoration: the scanner reads `name`/`description`/`url` out of the
    /// agent card, `skills` out of the skills index and `linkset` out of the
    /// catalog. A document that parses but lost one of those keys fails without
    /// looking broken.
    #[tokio::test]
    async fn the_json_surfaces_carry_the_fields_consumers_index_on() {
        let required: &[(&str, &[&str])] = &[
            (
                "/.well-known/agent-card.json",
                &["name", "description", "url"],
            ),
            ("/.well-known/agent.json", &["name", "description", "url"]),
            ("/.well-known/agent-skills/index.json", &["skills"]),
            ("/.well-known/api-catalog", &["linkset"]),
            ("/.well-known/x402", &["x402"]),
            ("/.well-known/oauth-protected-resource", &["resource"]),
            ("/workflows.json", &["workflows"]),
        ];
        for (path, keys) in required {
            let (_, _, body) = fetch(path).await;
            let doc: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path} is not JSON: {e}"));
            for key in *keys {
                assert!(
                    doc.get(key).is_some_and(|v| !v.is_null()),
                    "{path} is missing the `{key}` field"
                );
            }
        }
    }

    /// The x402 document says what a facilitator can honestly say.
    ///
    /// `role: "facilitator"` and an empty `paidRoutes` are the whole point: this
    /// service takes no fee and none of its routes answer 402. If someone ever
    /// prices a route, this test is the reminder that the discovery document is
    /// now lying.
    #[tokio::test]
    async fn the_x402_document_declares_a_facilitator_that_charges_nothing() {
        let (_, _, body) = fetch("/.well-known/x402").await;
        let doc: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(doc["x402"]["role"], "facilitator");
        assert_eq!(
            doc["x402"]["paidRoutes"].as_array().map(|a| a.len()),
            Some(0),
            "the facilitator does not charge for its own routes"
        );
    }

    /// The legacy card is byte-identical to the current one.
    #[tokio::test]
    async fn the_legacy_agent_json_is_the_same_card() {
        let (_, _, card) = fetch("/.well-known/agent-card.json").await;
        let (_, _, legacy) = fetch("/.well-known/agent.json").await;
        assert_eq!(card, legacy, "the two A2A card locations disagree");
    }

    /// Normalise line endings before hashing or comparing.
    ///
    /// A Windows checkout stores these files with CRLF, and `include_str!` reads
    /// whatever is on disk. Production builds in a Linux container from an LF
    /// checkout, so LF is the form that is actually served and the form the
    /// published digest has to describe.
    fn lf(s: &str) -> String {
        s.replace("\r\n", "\n")
    }

    /// `llms-full.txt` still matches the four documents it is built from.
    ///
    /// It is generated by `scripts/build_llms_full.sh` precisely so it is not a
    /// fifth hand-maintained copy. This test is the other half of that: without
    /// it the generator is a suggestion, and the file silently rots the first
    /// time someone edits `skill.md` and does not rerun it.
    ///
    /// The format is duplicated here on purpose -- if you change the header or
    /// the separator in the script, change it here too. That is the cost of the
    /// check, and it is cheaper than the drift.
    #[test]
    fn llms_full_txt_is_in_sync_with_its_sources() {
        const HEADER: &str = concat!(
            "# x402 Payment Facilitator - Ultravioleta DAO - full agent context\n",
            "#\n",
            "# Generated by scripts/build_llms_full.sh. Do not edit by hand: edit the\n",
            "# sources below and re-run it. Sources, in order: static/llms.txt ",
            "static/index.md static/skill.md static/auth.md\n",
        );
        const SEPARATOR: &str = "\n---\n\n";

        let sources = [
            lf(include_str!("../static/llms.txt")),
            lf(include_str!("../static/index.md")),
            lf(include_str!("../static/skill.md")),
            lf(include_str!("../static/auth.md")),
        ];
        let expected = format!("{HEADER}{SEPARATOR}{}", sources.join(SEPARATOR));

        assert_eq!(
            lf(include_str!("../static/llms-full.txt")),
            expected,
            "static/llms-full.txt is stale -- run ./scripts/build_llms_full.sh and commit the result"
        );
    }

    /// The skills index publishes the real digest of `skill.md`.
    ///
    /// A `digest` that does not match is worse than no digest: a client that
    /// verifies it concludes the file was tampered with, and one that does not
    /// learns nothing. It has to be regenerated whenever `skill.md` changes,
    /// which is what this test enforces.
    #[test]
    fn the_skills_index_digest_matches_skill_md() {
        let index: serde_json::Value = serde_json::from_str(include_str!(
            "../static/.well-known/agent-skills/index.json"
        ))
        .expect("the skills index must be JSON");
        let published = index["skills"][0]["digest"]
            .as_str()
            .expect("the skill entry must publish a digest");

        let actual = format!(
            "sha256:{:x}",
            Sha256::digest(lf(include_str!("../static/skill.md")).as_bytes())
        );

        assert_eq!(
            published, actual,
            "the digest in .well-known/agent-skills/index.json is stale -- \
             skill.md changed and the index did not"
        );
    }

    /// The ARD catalog against the rules of the spec it claims to follow.
    ///
    /// The document is hand-written and the failure mode is silent: a registry
    /// that cannot parse an entry drops it and tells nobody. These are ARD
    /// v0.91 section 4.2 (the four required terms), 4.3 (url XOR data),
    /// Appendix C (the URN grammar and the publisher-domain anchor) and D.2
    /// (representativeQueries, 2-5 of them).
    #[test]
    fn the_ard_catalog_meets_the_spec() {
        const HOST: &str = "facilitator.ultravioletadao.xyz";
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../static/.well-known/ard.json"))
                .expect("ard.json must be JSON");

        let entries = doc["entries"]
            .as_array()
            .expect("an ARD manifest is an object with an `entries` array");
        assert!(!entries.is_empty(), "an empty catalog is worse than none");

        for entry in entries {
            let id = entry["identifier"]
                .as_str()
                .expect("4.2: `identifier` is required");

            // Appendix C: urn:air:<publisher>:<namespace>:<agent-name>, and the
            // publisher segment is the authority anchor -- claiming a domain
            // this host does not serve is what the grammar exists to prevent.
            let segments: Vec<&str> = id.split(':').collect();
            assert!(
                segments.len() >= 5 && segments[0] == "urn" && segments[1] == "air",
                "{id} is not urn:air:<publisher>:<namespace>:<name>"
            );
            assert_eq!(segments[2], HOST, "{id} claims a publisher we are not");
            assert!(
                segments[3..].iter().all(|s| !s.is_empty()),
                "{id} has an empty segment"
            );

            assert!(
                entry["displayName"].as_str().is_some_and(|v| !v.is_empty()),
                "4.2: {id} needs a displayName"
            );
            // 3.3: the type is an IANA media type, not a made-up token.
            assert!(
                entry["type"].as_str().is_some_and(|v| v.contains('/')),
                "4.2: {id} needs a media type"
            );

            // 4.3: exactly one of url / data. Both is invalid, neither is worse.
            let (has_url, has_data) = (!entry["url"].is_null(), !entry["data"].is_null());
            assert!(
                has_url ^ has_data,
                "4.3: {id} must carry exactly one of url/data"
            );
            if let Some(url) = entry["url"].as_str() {
                assert!(
                    url.starts_with(&format!("https://{HOST}/")),
                    "{id} points at {url}, which is not on this host"
                );
            }

            // D.2: without these an entry is a catalog listing, not a
            // discoverable one -- it simply never comes back from a search.
            let queries = entry["representativeQueries"]
                .as_array()
                .unwrap_or_else(|| panic!("D.2: {id} needs representativeQueries"));
            assert!(
                (2..=5).contains(&queries.len()),
                "D.2: {id} has {} representative queries, wanted 2-5",
                queries.len()
            );
        }

        // The four documents the catalog exists to advertise.
        let types: Vec<&str> = entries.iter().filter_map(|e| e["type"].as_str()).collect();
        for wanted in [
            "application/mcp-server-card+json",
            "application/a2a-agent-card+json",
            "application/ai-skill+md",
        ] {
            assert!(types.contains(&wanted), "the catalog must list a {wanted}");
        }
        assert!(
            types
                .iter()
                .any(|t| t.starts_with("application/vnd.oai.openapi+json")),
            "the catalog must list the OpenAPI document"
        );
    }

    /// `llms.txt` tells an agent WHEN to reach for this service.
    ///
    /// `agent-instruction` is a required check on both scanners and it failed
    /// on 2026-09-02: the file described what the service is at length and
    /// never said what jobs it is the right answer to. An agent choosing among
    /// ten tools picks the one that says what it is for -- and, just as
    /// usefully, the one that rules itself out.
    ///
    /// This asserts the section exists and still names the three things this
    /// service is most often mistaken for. It is a shape check, not a prose
    /// check: it fails when the section is deleted or gutted, which is the
    /// regression that matters.
    #[test]
    fn llms_txt_says_when_to_use_this_service_and_when_not_to() {
        let llms = include_str!("../static/llms.txt");
        let heading = llms
            .lines()
            .position(|line| {
                let l = line.to_ascii_lowercase();
                l.starts_with("##") && l.contains("when to use")
            })
            .expect("llms.txt needs a `when to use this` section");

        // Early enough to be read before the reference material.
        assert!(
            heading < 40,
            "the guidance sits {heading} lines in; an agent skimming the top \
             of the file will not reach it"
        );

        let section: String = llms
            .lines()
            .skip(heading)
            .take_while(|line| {
                !(line.starts_with("## ") && !line.to_ascii_lowercase().contains("when to use"))
            })
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();

        // The best-fit jobs, named as calls an agent can make.
        for call in ["/verify", "/settle", "/supported", "/mcp"] {
            assert!(
                section.contains(call),
                "the guidance must name {call} as a job this service does"
            );
        }
        // The three things it is most often mistaken for. Ruling yourself out
        // is the half that stops an agent wasting a call.
        for not in ["wallet", "marketplace", "ledger"] {
            assert!(
                section.contains(not),
                "the guidance must say this is not a {not}"
            );
        }
        assert!(
            section.contains("do **not**") || section.contains("not to"),
            "the guidance needs an explicit negative half"
        );
    }

    /// Every url in the sitemap carries a parseable `<lastmod>`.
    ///
    /// ora.ai reported "none of the 10 sampled urls has a lastmod": without one
    /// a crawler cannot tell a document that moved this morning from one
    /// untouched since July, so it either re-reads everything or nothing.
    ///
    /// TO RECOMPUTE A DATE, use the commit date of the file that BACKS the url,
    /// not the date you edited the sitemap:
    ///
    /// ```text
    /// git log -1 --format=%cI -- static/skill.md
    /// ```
    ///
    /// The mapping is not derivable from the url, which is why it is written
    /// out here:
    ///
    /// | url | file |
    /// |---|---|
    /// | `/` | `static/index.html` |
    /// | `/docs` | `src/openapi.rs` |
    /// | `/mcp` | `static/mcp.html` (Markdown: `static/mcp.md`) |
    /// | `/networks` | `static/networks.html` |
    /// | `/x402` | `static/x402.html` |
    /// | `/bazaar` | `static/bazaar.html` |
    /// | `/stats` | `static/stats.html` |
    /// | `/events/live` | `static/events-viewer.html` |
    /// | `/llms.txt` | `static/llms.txt` |
    /// | `/llms-full.txt` | `static/llms-full.txt` |
    /// | `/index.md` | `static/index.md` |
    /// | `/skill.md` | `static/skill.md` |
    /// | `/auth.md` | `static/auth.md` |
    ///
    /// Deliberately NOT a freshness check. Asserting that the newest date is
    /// recent would turn every quiet week into a red build, and the thing that
    /// actually breaks is a url added without a date -- which this catches.
    #[test]
    fn the_sitemap_stamps_every_url() {
        let sitemap = include_str!("../static/sitemap.xml");
        // Everything after the leading comment, so the prose above cannot be
        // mistaken for markup.
        let body = sitemap
            .split_once("<urlset")
            .expect("the sitemap needs a <urlset>")
            .1;

        let blocks: Vec<&str> = body.split("<url>").skip(1).collect();
        assert!(!blocks.is_empty(), "the sitemap lists no urls");

        for block in &blocks {
            let loc = block
                .split_once("<loc>")
                .and_then(|(_, rest)| rest.split_once("</loc>"))
                .map(|(value, _)| value.trim())
                .expect("every <url> needs a <loc>");

            let lastmod = block
                .split_once("<lastmod>")
                .and_then(|(_, rest)| rest.split_once("</lastmod>"))
                .map(|(value, _)| value.trim())
                .unwrap_or_else(|| {
                    panic!(
                        "{loc} has no <lastmod>. Add one: the commit date of the \
                         file that backs it, `git log -1 --format=%cI -- <file>`. \
                         The mapping is in this test's doc comment."
                    )
                });

            // W3C datetime: a date, optionally with a time and offset. Checked
            // by shape rather than parsed -- a typo'd month is the realistic
            // failure, and `2026-13-01` is not a date.
            let (date, _) = lastmod.split_once('T').unwrap_or((lastmod, ""));
            let parts: Vec<&str> = date.split('-').collect();
            assert_eq!(parts.len(), 3, "{loc}: {lastmod} is not a W3C date");
            let year: i32 = parts[0]
                .parse()
                .unwrap_or_else(|_| panic!("{loc}: {lastmod}"));
            let month: u32 = parts[1]
                .parse()
                .unwrap_or_else(|_| panic!("{loc}: {lastmod}"));
            let day: u32 = parts[2]
                .parse()
                .unwrap_or_else(|_| panic!("{loc}: {lastmod}"));
            assert!(year >= 2025, "{loc}: {lastmod} predates this repository");
            assert!(
                (1..=12).contains(&month),
                "{loc}: month {month} in {lastmod}"
            );
            assert!((1..=31).contains(&day), "{loc}: day {day} in {lastmod}");
        }
    }

    /// Everything that links to another surface links to one that exists.
    ///
    /// A catalog pointing at a 404 is the failure mode this whole set of files
    /// is meant to avoid, and it is invisible unless something walks the links.
    #[tokio::test]
    async fn every_internal_link_resolves_to_a_served_route() {
        const HOST: &str = "https://facilitator.ultravioletadao.xyz";
        // Routes served elsewhere in the router that these documents may link to.
        const SERVED_ELSEWHERE: &[&str] = &[
            "/",
            "/docs",
            "/health",
            "/version",
            "/supported",
            "/verify",
            "/settle",
            "/accepts",
            "/openapi.json",
            "/bazaar",
            "/stats",
            "/events",
            "/events/live",
            "/transactions",
            "/api/stats",
            "/blacklist",
            "/escrow/state",
            "/register",
            "/mcp",
            "/networks",
            "/x402",
        ];

        for (path, _) in SURFACES {
            let (_, _, body) = fetch(path).await;
            for raw in body.split(HOST).skip(1) {
                let link: String = raw
                    .chars()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | ')' | '`' | ',' | '<'))
                    .collect();
                let link = link.trim_end_matches(['.', ':', ';']);
                if link.is_empty() || link.contains('{') {
                    continue;
                }
                let known =
                    SURFACES.iter().any(|(p, _)| *p == link) || SERVED_ELSEWHERE.contains(&link);
                assert!(known, "{path} links to {HOST}{link}, which no route serves");
            }
        }
    }
}

/// Item A of the 2026-09-02 agentic wave: `Accept` negotiation on the four
/// surfaces that have a Markdown representation, and on `/`.
///
/// These run against the real routers rather than calling the handlers, because
/// the failure that matters is a header -- and a handler tested in isolation
/// still carries the right header while the route drops it.
#[cfg(test)]
mod markdown_negotiation_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// `(status, content-type, every Vary value joined, body)`.
    async fn fetch(
        router: Router,
        path: &str,
        accept: Option<&str>,
    ) -> (StatusCode, String, String, String) {
        let mut builder = Request::builder().uri(path);
        if let Some(accept) = accept {
            builder = builder.header("accept", accept);
        }
        let response = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let header = |name: &str| {
            response
                .headers()
                .get_all(name)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let ctype = header("content-type");
        let vary = header("vary");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            ctype,
            vary,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    fn root_router() -> Router {
        Router::new().route("/", get(get_root))
    }

    /// The default has to be unchanged: this route is the landing page, and a
    /// browser that sends no `Accept` at all must not get Markdown.
    #[tokio::test]
    async fn the_root_serves_html_by_default_and_markdown_on_request() {
        let (status, ctype, vary, body) = fetch(root_router(), "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(ctype.starts_with("text/html"), "default was {ctype}");
        assert!(body.contains("<!DOCTYPE html>") || body.contains("<!doctype html>"));
        assert!(
            vary.to_ascii_lowercase().contains("accept"),
            "Vary must list Accept even on the HTML branch, or a cache serves \
             one representation to both audiences; got {vary:?}"
        );

        let (status, ctype, vary, body) = fetch(root_router(), "/", Some("text/markdown")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ctype, "text/markdown; charset=utf-8");
        assert!(vary.to_ascii_lowercase().contains("accept"));
        assert_eq!(body, INDEX_MD, "/ must serve /index.md byte for byte");
    }

    /// The header that would break a substring implementation.
    #[tokio::test]
    async fn a_browsers_accept_header_still_gets_html() {
        let chrome = "text/html,application/xhtml+xml,application/xml;q=0.9,\
                      image/avif,image/webp,image/apng,*/*;q=0.8";
        let (_, ctype, _, _) = fetch(root_router(), "/", Some(chrome)).await;
        assert!(ctype.starts_with("text/html"), "got {ctype}");
        // curl's default, and a client that sends nothing meaningful.
        let (_, ctype, _, _) = fetch(root_router(), "/", Some("*/*")).await;
        assert!(ctype.starts_with("text/html"), "got {ctype}");
    }

    /// Every negotiated surface carries `Vary: Accept` -- the whole of the
    /// `markdown-negotiation-vary` check.
    #[tokio::test]
    async fn every_negotiated_surface_varies_on_accept() {
        for path in ["/llms.txt", "/index.md", "/skill.md", "/auth.md"] {
            for accept in [None, Some("text/markdown")] {
                let (status, _, vary, _) = fetch(agentic_routes(), path, accept).await;
                assert_eq!(status, StatusCode::OK, "{path}");
                assert!(
                    vary.to_ascii_lowercase().contains("accept"),
                    "{path} with Accept={accept:?} answered Vary={vary:?}"
                );
            }
        }
    }

    /// `/llms.txt` keeps its `text/plain` default -- the readiness checker and
    /// every existing consumer read that path expecting plain text -- and
    /// relabels the same bytes for an agent that asks for Markdown.
    #[tokio::test]
    async fn llms_txt_keeps_text_plain_by_default() {
        let (_, ctype, _, plain) = fetch(agentic_routes(), "/llms.txt", None).await;
        assert_eq!(ctype, "text/plain; charset=utf-8");
        let (_, ctype, _, md) = fetch(agentic_routes(), "/llms.txt", Some("text/markdown")).await;
        assert_eq!(ctype, "text/markdown; charset=utf-8");
        assert_eq!(plain, md, "the bytes must not depend on the label");
    }

    /// The 406 branch, and the reason it does not break the humans who click
    /// the `/skill.md` link out of `llms.txt`.
    ///
    /// A `.md` path has exactly one representation, so RFC 9110 section 15.5.7
    /// says an `Accept` that cannot be satisfied earns a 406 -- and
    /// acceptmarkdown.com grades that. The reason that is safe here rather than
    /// hostile is measured below: every real browser ends its `Accept` with a
    /// `*/*` entry, which matches Markdown, so only a client that names
    /// `text/html` and nothing else -- a header no browser sends -- reaches the
    /// refusal. If that ever stops being true, this test is where it shows.
    #[tokio::test]
    async fn a_browser_reaches_the_document_and_only_a_true_refusal_earns_406() {
        let chrome = "text/html,application/xhtml+xml,application/xml;q=0.9,\
                      image/avif,image/webp,image/apng,*/*;q=0.8";
        let firefox = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
        let safari = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
        for header in [chrome, firefox, safari, "*/*", "text/*"] {
            let (status, ctype, _, _) = fetch(agentic_routes(), "/skill.md", Some(header)).await;
            assert_eq!(status, StatusCode::OK, "Accept={header:?} was refused");
            assert_eq!(ctype, "text/markdown; charset=utf-8");
        }

        // No wildcard anywhere and Markdown unnamed: nothing on offer matches.
        for header in ["text/html", "application/pdf", "text/markdown;q=0"] {
            let (status, ctype, vary, body) =
                fetch(agentic_routes(), "/skill.md", Some(header)).await;
            assert_eq!(status, StatusCode::NOT_ACCEPTABLE, "Accept={header:?}");
            assert!(ctype.starts_with("application/json"), "got {ctype}");
            assert!(vary.to_ascii_lowercase().contains("accept"));
            assert!(
                body.contains("text/markdown"),
                "a 406 must name what it can serve; got {body}"
            );
        }
    }
}

/// Item B: the 404 an agent can recover from.
#[cfg(test)]
mod agent_404_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn fetch(
        router: Router,
        path: &str,
        accept: Option<&str>,
    ) -> (StatusCode, String, String) {
        let mut builder = Request::builder().uri(path);
        if let Some(accept) = accept {
            builder = builder.header("accept", accept);
        }
        let response = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let ctype = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, ctype, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn router() -> Router {
        Router::new().fallback(agent_not_found)
    }

    #[tokio::test]
    async fn an_unknown_path_answers_404_markdown_that_names_where_to_look() {
        let (status, ctype, body) = fetch(router(), "/no-existe", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(ctype, "text/markdown; charset=utf-8");
        assert!(
            body.starts_with("# "),
            "a markdown body has to open on a heading; got {:?}",
            &body[..body.len().min(40)]
        );
        // The recovery path is the whole reason this body exists.
        for link in [
            "/llms.txt",
            "/sitemap.xml",
            "/openapi.json",
            "/.well-known/api-catalog",
        ] {
            assert!(body.contains(link), "the 404 must point at {link}");
        }
    }

    #[tokio::test]
    async fn a_json_caller_gets_a_typed_json_404() {
        let (status, ctype, body) = fetch(router(), "/no-existe", Some("application/json")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(ctype.starts_with("application/json"), "got {ctype}");
        let doc: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(doc["code"], "not_found");
        assert!(doc["error"].is_string());
        assert!(doc["hint"].as_str().unwrap().contains("llms.txt"));
    }

    /// A 404 never becomes a 406: the caller already asked for something that
    /// does not exist, and refusing to say so is a worse dead end.
    #[tokio::test]
    async fn an_impossible_accept_still_gets_the_404() {
        for accept in ["application/pdf", "text/markdown;q=0, application/json;q=0"] {
            let (status, ctype, _) = fetch(router(), "/no-existe", Some(accept)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "Accept={accept:?}");
            assert!(ctype.starts_with("text/markdown"), "got {ctype}");
        }
    }

    /// The regression this is really guarding: axum keeps the fallback of the
    /// router merged LAST, and `.layer()` wraps a router's default fallback
    /// along with its routes. Before this change the 404 belonged to whichever
    /// governed router happened to be merged last -- an accident that a
    /// reordering could change silently. Merging a router with a real route
    /// AFTER the fallback router must not take the fallback back.
    #[tokio::test]
    async fn the_fallback_survives_being_merged_with_other_routers() {
        let assembled = Router::new()
            .merge(agentic_routes())
            .merge(Router::new().fallback(agent_not_found))
            .merge(Router::new().route("/health-probe", get(|| async { "ok" })));
        let (status, ctype, body) = fetch(assembled, "/definitely-not-a-route", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(ctype, "text/markdown; charset=utf-8");
        assert!(body.contains("/llms.txt"));
    }
}

/// Items C and D: every refusal is typed JSON, and the budget is legible.
#[cfg(test)]
mod json_error_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// A router shaped like `main.rs`: real routes, then the 405 fallback last.
    fn router() -> Router {
        Router::new()
            .route("/thing", get(|| async { "ok" }))
            .route("/writable", post(|| async { "ok" }))
            .method_not_allowed_fallback(method_not_allowed)
    }

    async fn call(method: &str, path: &str) -> (StatusCode, String, String, String) {
        let response = router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let get = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let (ctype, allow) = (get("content-type"), get("allow"));
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            ctype,
            allow,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    /// The gap `json-error-responses` actually scored: a 405 with zero bytes
    /// and no content type.
    #[tokio::test]
    async fn a_wrong_method_is_a_typed_json_405() {
        let (status, ctype, _, body) = call("DELETE", "/thing").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(ctype.starts_with("application/json"), "got {ctype}");
        let doc: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(doc["code"], "method_not_allowed");
        assert!(doc["error"].as_str().unwrap().contains("DELETE"));
        assert!(doc["error"].as_str().unwrap().contains("/thing"));
        assert!(doc["hint"].as_str().unwrap().contains("openapi.json"));
    }

    /// The custom fallback must not cost the `Allow` header: it is the only
    /// part of a 405 a client can act on without reading prose.
    #[tokio::test]
    async fn the_405_still_carries_allow() {
        let (_, _, allow, _) = call("DELETE", "/thing").await;
        assert_eq!(allow, "GET,HEAD");
        let (_, _, allow, _) = call("GET", "/writable").await;
        assert_eq!(allow, "POST");
    }

    /// tower_governor builds both refusals with `Response::new(String)` and no
    /// content type. The status and every header it set have to survive being
    /// retyped -- `retry-after` is the whole point of a 429.
    #[test]
    fn a_rate_limit_refusal_becomes_json_without_losing_its_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("7"));
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("30"));
        let response = rate_limit_error(tower_governor::GovernorError::TooManyRequests {
            wait_time: 7,
            headers: Some(headers),
        });
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "7");
        assert_eq!(response.headers()["x-ratelimit-limit"], "30");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            APPLICATION_JSON_UTF8
        );
    }

    #[tokio::test]
    async fn the_rate_limit_body_is_json_with_a_code_and_a_hint() {
        let response = rate_limit_error(tower_governor::GovernorError::TooManyRequests {
            wait_time: 7,
            headers: None,
        });
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(doc["code"], "rate_limited");
        assert!(doc["hint"].as_str().unwrap().contains("retry-after"));

        // The 500 branch: it only fires on a direct connection, but an untyped
        // 500 is the worst of the three to hand an agent.
        let response = rate_limit_error(tower_governor::GovernorError::UnableToExtractKey);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            APPLICATION_JSON_UTF8
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(doc["code"], "rate_limit_key_unavailable");
    }
}

/// The bilingual pages, checked as a contract instead of by eye.
///
/// Every human page here carries BOTH languages in one document at one URL:
/// there is no `/es/`, no `hreflang` and no per-language sitemap. That choice
/// buys a single canonical URL and pays for it with a failure mode that is
/// completely invisible: `updateTranslations` leaves the hardcoded English
/// markup in place when a key is missing, so a page with a hole in its Spanish
/// dictionary renders perfectly and simply stops switching language. Seven keys
/// went unnoticed that way before anyone looked.
///
/// Two invariants close it:
///
///   * **N1, parity** -- every key exists in `en` AND in `es`.
///   * **N2, coverage** -- every key the markup asks for is defined in both.
///
/// Both are verified by mutation, not by colour: deleting a single key from one
/// dictionary must turn one of them red. See the 2026-09-02 handoff for the run.
#[cfg(test)]
mod i18n_tests {
    use super::{
        BAZAAR_HTML, EVENTS_VIEWER_HTML, INDEX_HTML, MCP_HTML, NETWORKS_HTML, STATS_HTML, X402_HTML,
    };
    use std::collections::BTreeSet;

    /// The pages and the `const` name of the JS object holding their dictionary.
    ///
    /// The landing calls it `translations` and the three smaller pages call it
    /// `I18N`; both spellings are listed rather than unified because renaming a
    /// live page's variable to please a test is the wrong direction.
    const PAGES: &[(&str, &str)] = &[
        ("static/index.html", INDEX_HTML),
        ("static/bazaar.html", BAZAAR_HTML),
        ("static/stats.html", STATS_HTML),
        ("static/events-viewer.html", EVENTS_VIEWER_HTML),
        ("static/mcp.html", MCP_HTML),
        ("static/networks.html", NETWORKS_HTML),
        ("static/x402.html", X402_HTML),
    ];

    /// The three attributes that make the runtime look a key up.
    ///
    /// `data-i18n-ph` is the one that is easy to forget: only `bazaar.html` uses
    /// it, for a `placeholder`, and a checker that scanned the other two would
    /// pass while that string stayed monolingual.
    const ATTRIBUTES: &[&str] = &["data-i18n=\"", "data-i18n-html=\"", "data-i18n-ph=\""];

    /// The inside of the first `{...}` at or after `from`, brace-matched with
    /// string and comment awareness.
    ///
    /// A plain `find('}')` would stop at the first `}` inside a translated
    /// string -- and several values here carry inline HTML with braces in their
    /// `style` attributes.
    fn brace_block(src: &str, from: usize) -> Option<&str> {
        let bytes: Vec<char> = src.chars().collect();
        let mut idx: Vec<usize> = Vec::with_capacity(bytes.len() + 1);
        let mut acc = 0usize;
        for c in &bytes {
            idx.push(acc);
            acc += c.len_utf8();
        }
        idx.push(acc);

        let mut i = src[..from].chars().count();
        while i < bytes.len() && bytes[i] != '{' {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let start = i + 1;
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        while i < bytes.len() {
            let c = bytes[i];
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
                i += 1;
                continue;
            }
            match c {
                '"' | '\'' | '`' => quote = Some(c),
                '/' if bytes.get(i + 1) == Some(&'/') => {
                    while i < bytes.len() && bytes[i] != '\n' {
                        i += 1;
                    }
                    continue;
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&src[idx[start]..idx[i]]);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The keys declared at the top level of one language object.
    ///
    /// Written as a scanner rather than a regex because a regex over `"([^"]+)":`
    /// also matches inside a translated value -- and this file has values that
    /// carry inline `style="..."` attributes with colons in them, which is
    /// exactly how a checker ends up inventing keys nobody wrote.
    fn dict_keys(block: &str) -> BTreeSet<String> {
        let chars: Vec<char> = block.chars().collect();
        let mut keys = BTreeSet::new();
        let mut i = 0usize;
        let mut depth = 0i32;
        let mut expect_key = true;

        let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '.';

        while i < chars.len() {
            let c = chars[i];
            if c == '/' && chars.get(i + 1) == Some(&'/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            if c == '"' || c == '\'' || c == '`' {
                let quote = c;
                let start = i + 1;
                let mut j = start;
                let mut escaped = false;
                while j < chars.len() {
                    if escaped {
                        escaped = false;
                    } else if chars[j] == '\\' {
                        escaped = true;
                    } else if chars[j] == quote {
                        break;
                    }
                    j += 1;
                }
                let text: String = chars[start..j.min(chars.len())].iter().collect();
                i = j + 1;
                if depth == 0 && expect_key {
                    let mut k = i;
                    while k < chars.len() && chars[k].is_whitespace() {
                        k += 1;
                    }
                    if chars.get(k) == Some(&':') {
                        keys.insert(text);
                        i = k + 1;
                        expect_key = false;
                    }
                }
                continue;
            }
            match c {
                '{' | '[' => {
                    depth += 1;
                    i += 1;
                }
                '}' | ']' => {
                    depth -= 1;
                    i += 1;
                }
                ',' => {
                    if depth == 0 {
                        expect_key = true;
                    }
                    i += 1;
                }
                _ if depth == 0 && expect_key && is_ident(c) => {
                    let start = i;
                    while i < chars.len() && is_ident(chars[i]) {
                        i += 1;
                    }
                    let mut k = i;
                    while k < chars.len() && chars[k].is_whitespace() {
                        k += 1;
                    }
                    if chars.get(k) == Some(&':') {
                        keys.insert(chars[start..i].iter().collect());
                        i = k + 1;
                        expect_key = false;
                    }
                }
                _ => i += 1,
            }
        }
        keys
    }

    /// `(english keys, spanish keys)` for one page.
    fn dictionaries(page: &str, html: &str) -> (BTreeSet<String>, BTreeSet<String>) {
        let open = html
            .find("const translations = {")
            .or_else(|| html.find("const I18N = {"))
            .unwrap_or_else(|| panic!("{page} has no `const translations` / `const I18N` object"));
        let dict = brace_block(html, open)
            .unwrap_or_else(|| panic!("{page}: the dictionary object is not brace-balanced"));

        let lang = |name: &str| -> BTreeSet<String> {
            // The first `<lang>:` that is followed by an object is the language
            // block; a translated value containing the same two characters is
            // not followed by a `{`.
            let mut from = 0usize;
            loop {
                let at = dict[from..]
                    .find(&format!("{name}:"))
                    .unwrap_or_else(|| panic!("{page} has no `{name}:` dictionary"))
                    + from;
                let after = at + name.len() + 1;
                if dict[after..].trim_start().starts_with('{') {
                    let block = brace_block(dict, after).unwrap_or_else(|| {
                        panic!("{page}: the `{name}` dictionary is not brace-balanced")
                    });
                    return dict_keys(block);
                }
                from = after;
            }
        };
        (lang("en"), lang("es"))
    }

    /// Every key the markup asks for, across the three lookup attributes.
    fn keys_used(html: &str) -> BTreeSet<String> {
        let mut used = BTreeSet::new();
        for attribute in ATTRIBUTES {
            for chunk in html.split(attribute).skip(1) {
                if let Some((key, _)) = chunk.split_once('"') {
                    used.insert(key.to_string());
                }
            }
        }
        used
    }

    /// N1. A key in one language and not the other is a page that renders fine
    /// and silently refuses to translate that one string.
    #[test]
    fn every_key_exists_in_both_languages() {
        for (page, html) in PAGES {
            let (en, es) = dictionaries(page, html);
            assert!(!en.is_empty(), "{page}: the `en` dictionary parsed as empty");
            assert!(!es.is_empty(), "{page}: the `es` dictionary parsed as empty");

            let only_en: Vec<&String> = en.difference(&es).collect();
            let only_es: Vec<&String> = es.difference(&en).collect();
            assert!(
                only_en.is_empty(),
                "{page}: {only_en:?} exist in `en` and not in `es`. A new string \
                 goes into BOTH dictionaries in the same commit -- in Spanish the \
                 page would keep showing the English markup and never say why."
            );
            assert!(
                only_es.is_empty(),
                "{page}: {only_es:?} exist in `es` and not in `en`. The English \
                 side is the one search engines index; a key missing there is a \
                 string with no canonical form."
            );
        }
    }

    /// N2. A `data-i18n` pointing at a key nobody defined is the failure that
    /// leaves the hardcoded markup on screen in every language.
    #[test]
    fn every_key_used_by_the_markup_is_defined_in_both_languages() {
        for (page, html) in PAGES {
            let (en, es) = dictionaries(page, html);
            let used = keys_used(html);
            assert!(!used.is_empty(), "{page}: no data-i18n attributes found");

            let missing_en: Vec<&String> = used.difference(&en).collect();
            let missing_es: Vec<&String> = used.difference(&es).collect();
            assert!(
                missing_en.is_empty(),
                "{page}: the markup asks for {missing_en:?}, undefined in `en`"
            );
            assert!(
                missing_es.is_empty(),
                "{page}: the markup asks for {missing_es:?}, undefined in `es`"
            );
        }
    }

    /// One storage key for the whole site.
    ///
    /// `bazaar.html` used to write `uvd-lang` while the other three wrote
    /// `x402.lang`, so choosing Spanish on one side and navigating to the other
    /// silently reset the choice. The legacy name may only survive as the
    /// migration constant that moves an existing visitor's value across.
    #[test]
    fn the_language_choice_lives_under_one_key() {
        for (page, html) in PAGES {
            assert!(
                html.contains("x402.lang"),
                "{page} does not use the shared `x402.lang` storage key"
            );
        }
        let legacy = BAZAAR_HTML.matches("uvd-lang").count();
        assert_eq!(
            legacy, 2,
            "static/bazaar.html mentions `uvd-lang` {legacy} times; expected exactly \
             two -- the comment explaining the migration and the LEGACY_LANG_KEY \
             constant that performs it. A third is a write that resurrects the split."
        );
    }

    /// The browser's language does not choose. An explicit click does.
    ///
    /// One URL per page means the language has to be something a reader can see
    /// and undo. Deciding it from `navigator.language` serves a different
    /// document to different readers at the same address with nothing to point
    /// at -- and it is the exact behaviour the owner ruled out on 2026-09-02.
    #[test]
    fn no_page_picks_a_language_from_the_browser() {
        for (page, html) in PAGES {
            // Whole-line `//` comments are dropped first, so a page is free to
            // explain in prose the very API it must not call -- and `mcp.html`
            // does, which is how this line got written. The check is about code.
            let code: String = html
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("navigator.language"),
                "{page} still reads navigator.language to pick a language"
            );
        }
    }

    /// The canonical `<title>` in the markup is English, and it is translatable.
    ///
    /// Both halves matter and they pull in opposite directions: a crawler reads
    /// the literal in the file, so it has to be English, while a reader who
    /// picked Spanish should see a Spanish tab. `data-i18n` on the tag is what
    /// buys the second without giving up the first.
    #[test]
    fn every_page_title_is_english_and_translatable() {
        for (page, html) in PAGES {
            let title = html
                .split_once("<title")
                .and_then(|(_, rest)| rest.split_once("</title>"))
                .map(|(open, _)| open)
                .unwrap_or_else(|| panic!("{page} has no <title>"));
            assert!(
                title.contains("data-i18n=\""),
                "{page}: <title> carries no data-i18n, so the tab stays English \
                 for a reader who chose Spanish"
            );
            let text = title.split_once('>').map(|(_, t)| t).unwrap_or("");
            for spanish_only in ["métricas", "en vivo", "Bazar "] {
                assert!(
                    !text.contains(spanish_only),
                    "{page}: <title> markup reads {text:?}, which is Spanish. The \
                     literal in the file is what a crawler indexes and it is \
                     canonical English; the Spanish lives in the `es` dictionary."
                );
            }
        }
    }
}
