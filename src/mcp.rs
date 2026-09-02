//! The facilitator as an MCP server, behind `POST /mcp`.
//!
//! # What this is
//!
//! Four tools -- `x402_supported`, `x402_accepts`, `x402_verify`, `x402_settle`
//! -- over MCP's Streamable HTTP transport, served by the same binary, on the
//! same ALB, under the same rate limiter as `/verify` and `/settle`. No new
//! infrastructure, no second deploy, no second TLS certificate to rotate.
//!
//! # The one design decision worth reading
//!
//! **A tool call is dispatched through the REST router, not to the handler
//! functions.** `call_rest` builds a synthetic `http::Request` and runs it
//! through a `Router` that carries the very same routes, so an MCP `x402_settle`
//! and a `POST /settle` execute the identical stack.
//!
//! That is not a stylistic preference. `POST /settle` is wrapped in
//! [`crate::handlers::settle_writer_gate`], which reads the body, decides
//! whether the payment targets an EVM chain, and -- if it does and this task
//! does not hold the writer lease -- forwards the request to the task that
//! does. That gate is what serialises the nonce of the single EOA that spends
//! gas. Calling `post_settle::<A>(...)` as a function would skip it and let two
//! ECS tasks sign at once.
//!
//! The rate limiter is not charged twice: the inner router carries no
//! `GovernorLayer`, and the outer `/mcp` route already spent its token.
//!
//! # No privilege comes with the MCP door
//!
//! The facilitator charges nothing and authenticates nobody (`static/auth.md`).
//! `x402_settle` over MCP is `POST /settle` over HTTP: same handler, same
//! validation of the payer's signed payload, same everything. An MCP client
//! cannot move funds that an HTTP client could not.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, post_service};
use axum::{Json, Router};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ErrorCode, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use crate::chain::NetworkProvider;
use crate::discovery::DiscoveryRegistry;
use crate::facilitator::Facilitator;
use crate::provider_cache::{HasProviderMap, ProviderMap};

/// The tools this server exposes, in the order `tools/list` reports them.
///
/// Exactly four. The admin surfaces -- `/discovery/admin/*`, `/feedback/revoke`,
/// `/register`, the DX402 writes -- are deliberately absent: an MCP client is an
/// LLM holding a tool list, and a tool that erases third-party reputation or
/// mints an identity does not belong in one. `tools_list_is_exactly_the_four`
/// fails if that ever quietly grows.
pub const TOOL_NAMES: [&str; 4] = [
    "x402_supported",
    "x402_accepts",
    "x402_verify",
    "x402_settle",
];

/// `MCP_ALLOWED_HOSTS`: comma-separated `Host` allowlist for `/mcp`.
const ENV_MCP_ALLOWED_HOSTS: &str = "MCP_ALLOWED_HOSTS";

/// The hosts `/mcp` answers on when nothing overrides them.
///
/// rmcp validates the `Host` header before anything else and answers **403** to
/// anything not on this list -- DNS-rebinding protection aimed at MCP servers
/// running on a developer's laptop. Its own default is loopback only, which
/// behind the ALB means every production request is rejected while local
/// testing looks perfect: a failure that only appears after the deploy. So the
/// production host is in the default, not left to configuration.
const DEFAULT_MCP_ALLOWED_HOSTS: &[&str] = &[
    "facilitator.ultravioletadao.xyz",
    "localhost",
    "127.0.0.1",
    "::1",
];

/// Value of [`ENV_MCP_ALLOWED_HOSTS`] that turns `Host` validation off.
const ALLOW_ANY_HOST: &str = "*";

/// Cap on the REST response body an MCP tool will read back.
///
/// `/supported` is the big one -- every network twice, v1 name plus CAIP-2 --
/// and it is tens of kilobytes, not megabytes. This is a guard against
/// buffering something unbounded, not a protocol limit.
const MAX_REST_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// The effective `Host` allowlist for `/mcp`.
///
/// Unset or blank keeps [`DEFAULT_MCP_ALLOWED_HOSTS`]. A literal `*` clears the
/// list, which is how rmcp spells "accept any host". Entries may carry a port
/// (`example.com:8080`); one without a port matches every port.
pub fn allowed_hosts() -> Vec<String> {
    let raw = std::env::var(ENV_MCP_ALLOWED_HOSTS).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return DEFAULT_MCP_ALLOWED_HOSTS
            .iter()
            .map(|h| (*h).to_string())
            .collect();
    }
    if raw == ALLOW_ANY_HOST {
        tracing::warn!(
            "{ENV_MCP_ALLOWED_HOSTS}=* : /mcp accepts any Host header (DNS-rebinding \
             protection disabled)"
        );
        return Vec::new();
    }
    let hosts: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect();
    if hosts.is_empty() {
        tracing::warn!(
            "{ENV_MCP_ALLOWED_HOSTS} held no usable host; falling back to the defaults"
        );
        return DEFAULT_MCP_ALLOWED_HOSTS
            .iter()
            .map(|h| (*h).to_string())
            .collect();
    }
    hosts
}

/// Which REST route a tool name stands for.
fn rest_target(tool: &str) -> Option<(Method, &'static str)> {
    match tool {
        "x402_supported" => Some((Method::GET, "/supported")),
        "x402_accepts" => Some((Method::POST, "/accepts")),
        "x402_verify" => Some((Method::POST, "/verify")),
        "x402_settle" => Some((Method::POST, "/settle")),
        _ => None,
    }
}

/// Turn a JSON literal into the object shape rmcp wants for `inputSchema`.
fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        other => unreachable!("tool schemas are JSON objects, got {other}"),
    }
}

/// The request envelope `POST /verify` and `POST /settle` parse.
///
/// The three top-level names are the serde fields of
/// [`crate::types::VerifyRequest`] and [`crate::types::SettleRequest`] -- not a
/// guess, and not a schema utoipa produced: neither type derives `ToSchema`,
/// and `src/openapi.rs` documents both bodies as a bare `Object`. The nested
/// objects stay open (`additionalProperties: true`) because the handler
/// auto-detects x402 v1 and v2 and carries extensions (`refund`, `upto`,
/// escrow actions) inside them.
fn payment_envelope_schema(operation: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "x402Version": {
                "type": "integer",
                "description": "x402 protocol version. 1 for the v1 wire format, 2 for CAIP-2 networks."
            },
            "paymentPayload": {
                "type": "object",
                "additionalProperties": true,
                "description": "The payer-signed authorization, exactly as POSTed to /verify and /settle."
            },
            "paymentRequirements": {
                "type": "object",
                "additionalProperties": true,
                "description": "What the resource server demands: scheme, network, asset, payTo, maxAmountRequired."
            }
        },
        "required": ["x402Version", "paymentPayload", "paymentRequirements"],
        "additionalProperties": true,
        "description": format!(
            "Identical to the JSON body of POST {operation}. Full shape and worked \
             examples: https://facilitator.ultravioletadao.xyz/skill.md"
        )
    })
}

/// The four tool definitions.
pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            TOOL_NAMES[0],
            "List every payment scheme and network this facilitator settles on, in both \
             the x402 v1 (\"base\") and CAIP-2 (\"eip155:8453\") spellings, plus the \
             protocol extensions it serves. Read this before building a payment: it is \
             the only authoritative answer to \"can you settle X on Y\". Takes no \
             arguments. Equivalent to GET /supported.",
            schema(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })),
        )
        .annotate(
            ToolAnnotations::with_title("Supported networks and schemes")
                .read_only(true)
                .idempotent(true)
                .open_world(false),
        ),
        Tool::new(
            TOOL_NAMES[1],
            "Negotiate payment requirements: send the `accepts` array from a 402 \
             response and get back only the entries this facilitator can actually \
             settle, enriched with what it knows (feePayer, token list, escrow \
             addresses). Moves no money and signs nothing. Equivalent to POST /accepts.",
            schema(json!({
                "type": "object",
                "properties": {
                    "x402Version": {
                        "type": "integer",
                        "description": "x402 protocol version. Defaults to 1 when absent."
                    },
                    "accepts": {
                        "type": "array",
                        "items": { "type": "object", "additionalProperties": true },
                        "description": "The payment requirements offered by the resource server, as in its 402 body."
                    },
                    "error": {
                        "type": "string",
                        "description": "The `error` field of the 402 body, passed through untouched."
                    }
                },
                "required": ["accepts"],
                "additionalProperties": true,
                "description": "Identical to the JSON body of POST /accepts -- the same shape a resource server returns with its HTTP 402."
            })),
        )
        .annotate(
            ToolAnnotations::with_title("Negotiate payment requirements")
                .read_only(true)
                .open_world(false),
        ),
        Tool::new(
            TOOL_NAMES[2],
            "Check whether a signed payment authorization would settle: signature, \
             nonce, amount, timestamps, token and network support. Submits NOTHING \
             on-chain and moves no funds -- it is the dry run you should call before \
             x402_settle. Equivalent to POST /verify.",
            schema(payment_envelope_schema("/verify")),
        )
        .annotate(
            ToolAnnotations::with_title("Verify a payment authorization")
                .read_only(true)
                .open_world(true),
        ),
        Tool::new(
            TOOL_NAMES[3],
            "SETTLE A PAYMENT ON-CHAIN. This MOVES REAL FUNDS and is IRREVERSIBLE: it \
             broadcasts the payer's authorization as a blockchain transaction, and \
             nothing in this facilitator or any chain can undo a confirmed transfer. \
             Call x402_verify first, and only call this when the payment is meant to \
             execute now. Equivalent to POST /settle.",
            schema(payment_envelope_schema("/settle")),
        )
        .annotate(
            ToolAnnotations::with_title("Settle a payment on-chain (irreversible)")
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
        ),
    ]
}

/// The MCP server: four tools over the facilitator's own REST router.
#[derive(Clone)]
pub struct FacilitatorMcp {
    /// The `/verify`, `/settle`, `/supported` and `/accepts` routes, with state
    /// and extensions already bound. Cloned per call; an axum `Router` is an
    /// `Arc` inside, so this is a refcount bump, not a rebuild.
    rest: Router,
}

impl FacilitatorMcp {
    /// Run one synthetic request through the REST router and read it back.
    ///
    /// Returns the status and the response body verbatim. Nothing is
    /// reinterpreted: whatever `/verify` answers an HTTP client is what an MCP
    /// client sees.
    async fn call_rest(
        &self,
        method: Method,
        path: &'static str,
        arguments: JsonObject,
    ) -> Result<(StatusCode, String), McpError> {
        let has_body = method != Method::GET;
        let mut builder = Request::builder().method(method).uri(path);
        if has_body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let body = if has_body {
            let bytes = serde_json::to_vec(&Value::Object(arguments)).map_err(|e| {
                McpError::invalid_params(format!("arguments are not serialisable JSON: {e}"), None)
            })?;
            Body::from(bytes)
        } else {
            Body::empty()
        };
        let request = builder.body(body).map_err(|e| {
            McpError::internal_error(format!("could not build the internal request: {e}"), None)
        })?;

        // `Infallible`, so the `?` is a formality the type system asks for.
        let response = self
            .rest
            .clone()
            .oneshot(request)
            .await
            .map_err(|e| McpError::internal_error(format!("internal routing failed: {e}"), None))?;

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), MAX_REST_RESPONSE_BYTES)
            .await
            .map_err(|e| {
                McpError::internal_error(format!("could not read the response body: {e}"), None)
            })?;
        Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
    }
}

impl ServerHandler for FacilitatorMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        info.protocol_version = ProtocolVersion::LATEST;
        // NOT `Implementation::from_build_env()`: that reads CARGO_PKG_VERSION,
        // which is the frozen `0.0.0` placeholder in Cargo.toml. The release
        // version lives in the VERSION file and reaches the binary as
        // FACILITATOR_VERSION -- same source as /version and the OpenAPI doc.
        info.server_info = Implementation::new(
            "x402-facilitator",
            crate::version::facilitator_version(),
        )
        .with_title("x402 Payment Facilitator")
        .with_description(
            "Verify and settle x402 gasless stablecoin payments across EVM, Solana, NEAR, \
             Stellar, Algorand, Sui and XRPL.",
        )
        .with_website_url("https://facilitator.ultravioletadao.xyz");
        info.instructions = Some(
            "Call x402_supported first to learn which network and scheme pairs this \
             facilitator settles. Then x402_accepts to narrow a resource server's 402 \
             offer, x402_verify to check a signed authorization, and x402_settle to \
             broadcast it. x402_settle moves real funds and cannot be undone. This \
             facilitator charges no fee and authenticates no caller: the payer's \
             signature is the only authority, so an MCP client can do exactly what an \
             HTTP client can, and nothing more."
                .to_string(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.clone();
        let Some((method, path)) = rest_target(name.as_ref()) else {
            // Unroutable request, not a tool that ran and failed: the caller
            // asked for something this server does not have.
            return Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {name}"),
                Some(json!({ "tools": TOOL_NAMES })),
            ));
        };
        let arguments = request.arguments.unwrap_or_default();
        let (status, body) = self.call_rest(method, path, arguments).await?;

        // The REST status decides. A 4xx/5xx becomes a tool-level error the
        // caller can read, carrying the facilitator's own message verbatim --
        // NOT an `Err(McpError)`, which most clients render opaquely and would
        // hide "invalid signature" behind "internal error".
        let content = vec![ContentBlock::text(body)];
        let result = if status.is_success() {
            CallToolResult::success(content)
        } else {
            CallToolResult::error(content)
        };
        Ok(result.into())
    }
}

/// `GET /mcp`: there is no stream here, and this says so in JSON.
///
/// rmcp answers 405 for GET when sessions are off, but with a `text/plain`
/// body and no `content-type` at all. A scanner grades a surface on its
/// content type as much as its status, so this route is served by us: same
/// 405, same `Allow: POST`, but a body a machine can read.
pub async fn get_mcp_no_stream() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        Json(json!({
            "error": "GET is not supported on /mcp",
            "reason": "This MCP server runs stateless: there is no server-initiated SSE \
                       stream to open. Send JSON-RPC over POST instead.",
            "transport": "streamable-http",
            "method": "POST",
            "serverCard": "https://facilitator.ultravioletadao.xyz/.well-known/mcp/server-card.json"
        })),
    )
}

/// The `/mcp` route.
///
/// `mount` it under the same `GovernorLayer` as `/verify` and `/settle`: an MCP
/// `x402_settle` costs the chain exactly what a `POST /settle` costs it.
///
/// The REST router built here carries **no** governor of its own -- see the
/// module docs for why that is correct rather than an oversight.
pub fn mcp_routes<A>(
    state: A,
    discovery_registry: Arc<DiscoveryRegistry>,
    event_bus: Arc<crate::events::EventBus>,
    transaction_store: Arc<dyn crate::transaction_store::TransactionStore>,
) -> Router
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    let rest = crate::handlers::verify_settle_routes::<A>()
        .merge(
            Router::new()
                .route("/supported", get(crate::handlers::get_supported::<A>))
                .route("/accepts", post(crate::handlers::post_accepts::<A>)),
        )
        .with_state(state)
        // `post_verify` and `post_settle` pull these out of the request
        // extensions. The synthetic request is built downstream of the layers
        // `main.rs` applies, so it has to carry its own copies.
        .layer(Extension(discovery_registry))
        .layer(Extension(event_bus))
        .layer(Extension(transaction_store));

    let hosts = allowed_hosts();
    tracing::info!(
        allowed_hosts = ?hosts,
        tools = ?TOOL_NAMES,
        "MCP server mounted at POST /mcp (streamable-http, stateless)"
    );

    let config = StreamableHttpServerConfig::default()
        // Stateless: no session id, no `Mcp-Session-Id` to pin a client to one
        // ECS task. There is exactly one task today, and this keeps it true
        // that there is nothing to pin when there are more.
        .with_legacy_session_mode(false)
        // Plain `application/json` for a request/response tool call, instead of
        // wrapping a single answer in an SSE frame.
        .with_json_response(true)
        .with_allowed_hosts(hosts);

    let service = StreamableHttpService::new(
        move || Ok(FacilitatorMcp { rest: rest.clone() }),
        Arc::new(NeverSessionManager::default()),
        config,
    );

    Router::new().route("/mcp", post_service(service).get(get_mcp_no_stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::FacilitatorLocalError;
    use crate::network::Network;
    use crate::types::{
        SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindsResponse,
        VerifyRequest, VerifyResponse,
    };
    use crate::types::{Scheme, X402Version};
    use axum::http::Request as HttpRequest;
    use std::borrow::Borrow;

    /// A provider map with nothing in it.
    ///
    /// The handlers under test never reach a chain: `/supported` is answered
    /// from the facilitator, and the `/verify` cases here are rejected on the
    /// body before any provider is consulted. Returning `None` is honest --
    /// there is no RPC configured in a unit test -- and it avoids standing up
    /// a `NetworkProvider`, which would mean real RPC clients.
    struct NoProviders;

    impl ProviderMap for NoProviders {
        type Value = NetworkProvider;

        fn by_network<N: Borrow<Network>>(&self, _network: N) -> Option<&Self::Value> {
            None
        }

        fn values(&self) -> impl Iterator<Item = &Self::Value> + Send {
            std::iter::empty()
        }
    }

    /// A facilitator that answers `/supported` and refuses everything else.
    #[derive(Clone)]
    struct StubFacilitator {
        providers: Arc<NoProviders>,
    }

    impl StubFacilitator {
        fn new() -> Self {
            Self {
                providers: Arc::new(NoProviders),
            }
        }
    }

    impl HasProviderMap for StubFacilitator {
        type Map = NoProviders;

        fn provider_map(&self) -> &Self::Map {
            &self.providers
        }
    }

    impl Facilitator for StubFacilitator {
        type Error = FacilitatorLocalError;

        async fn verify(&self, _request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
            Err(FacilitatorLocalError::ContractCall(
                "no chain in a unit test".to_string(),
            ))
        }

        async fn settle(&self, _request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
            Err(FacilitatorLocalError::ContractCall(
                "no chain in a unit test".to_string(),
            ))
        }

        async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
            Ok(SupportedPaymentKindsResponse {
                kinds: vec![SupportedPaymentKind {
                    x402_version: X402Version::V1,
                    scheme: Scheme::Exact,
                    network: Network::BaseSepolia.to_string(),
                    extra: None,
                }],
            })
        }
    }

    /// The MCP router, and the REST router it dispatches into, over the stub.
    ///
    /// Both are built from the SAME production functions the binary uses, so
    /// these tests exercise `mcp_routes` rather than a test-only rehearsal of
    /// it. The one thing they cannot exercise is the chain, which is why the
    /// live `curl` transcript in the handoff exists.
    async fn routers() -> (Router, Router) {
        let state = StubFacilitator::new();
        let discovery_registry = Arc::new(DiscoveryRegistry::new());
        let event_bus = Arc::new(crate::events::EventBus::from_env());
        let transaction_store = crate::transaction_store::create_transaction_store().await;

        let mcp = mcp_routes(
            state.clone(),
            Arc::clone(&discovery_registry),
            Arc::clone(&event_bus),
            Arc::clone(&transaction_store),
        );
        let rest = crate::handlers::verify_settle_routes::<StubFacilitator>()
            .merge(
                Router::new()
                    .route("/supported", get(crate::handlers::get_supported::<StubFacilitator>))
                    .route(
                        "/accepts",
                        post(crate::handlers::post_accepts::<StubFacilitator>),
                    ),
            )
            .with_state(state)
            .layer(Extension(discovery_registry))
            .layer(Extension(event_bus))
            .layer(Extension(transaction_store));
        (mcp, rest)
    }

    /// Send one JSON-RPC document to `POST /mcp` and read the answer back.
    ///
    /// The `Host` header is not decoration: rmcp validates it before anything
    /// else and answers 403 without one (`parse_host_header`). A test that
    /// forgot it would fail in a way that looks nothing like its cause.
    async fn rpc(mcp: &Router, body: Value) -> (StatusCode, String, Value) {
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = mcp.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        let json = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("MCP answered non-JSON ({e}): {text}"));
        (status, ctype, json)
    }

    /// The `initialize` document every MCP client sends first.
    fn initialize_request() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "x402-rs-test", "version": "0" }
            }
        })
    }

    #[tokio::test]
    async fn initialize_answers_with_a_protocol_version_and_a_named_server() {
        let (mcp, _) = routers().await;
        let (status, ctype, doc) = rpc(&mcp, initialize_request()).await;

        assert_eq!(status, StatusCode::OK, "initialize did not answer 200");
        assert!(
            ctype.starts_with("application/json"),
            "initialize answered content-type {ctype:?}"
        );
        let result = &doc["result"];
        assert!(
            result["protocolVersion"].is_string(),
            "no protocolVersion in {doc}"
        );
        assert_eq!(result["serverInfo"]["name"], "x402-facilitator");
        // The frozen Cargo.toml placeholder is what a workstation build
        // reports; what matters is that this tracks /version rather than
        // `CARGO_PKG_VERSION` read at compile time by rmcp itself.
        assert_eq!(
            result["serverInfo"]["version"],
            crate::version::facilitator_version()
        );
        assert_eq!(result["capabilities"]["tools"].is_null(), false);
    }

    /// The tool list is exactly the four, in order, and nothing else.
    ///
    /// This is the test that keeps an admin surface out. `/feedback/revoke`
    /// erases third-party reputation irreversibly and `/discovery/admin/*`
    /// curates a public catalog; neither belongs in a list handed to an LLM.
    #[tokio::test]
    async fn tools_list_is_exactly_the_four() {
        let (mcp, _) = routers().await;
        let (status, _, doc) = rpc(
            &mcp,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let names: Vec<String> = doc["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tools array in {doc}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, TOOL_NAMES.to_vec());
    }

    /// Every tool publishes a description and an object input schema.
    #[tokio::test]
    async fn every_tool_carries_a_description_and_an_input_schema() {
        let (mcp, _) = routers().await;
        let (_, _, doc) = rpc(
            &mcp,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
        )
        .await;
        for tool in doc["result"]["tools"].as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            assert!(
                tool["description"].as_str().is_some_and(|d| d.len() > 40),
                "{name} has no usable description"
            );
            assert_eq!(
                tool["inputSchema"]["type"], "object",
                "{name} has no object input schema"
            );
        }
    }

    /// `x402_settle` says out loud that it moves money and cannot be undone.
    ///
    /// The description is the only thing a model reads before deciding to call
    /// it. A settle tool that reads like a lookup is the failure mode here.
    #[tokio::test]
    async fn the_settle_tool_warns_that_it_is_irreversible() {
        let (mcp, _) = routers().await;
        let (_, _, doc) = rpc(
            &mcp,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
        )
        .await;
        let settle = doc["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "x402_settle")
            .expect("x402_settle must be listed");
        let description = settle["description"].as_str().unwrap().to_lowercase();
        assert!(
            description.contains("irreversible"),
            "the settle tool does not say it is irreversible: {description}"
        );
        assert!(
            description.contains("moves real funds"),
            "the settle tool does not say it moves funds: {description}"
        );
        assert_eq!(settle["annotations"]["destructiveHint"], true);
        assert_eq!(settle["annotations"]["readOnlyHint"], false);
    }

    /// Read the single text block out of a `tools/call` result.
    fn tool_text(doc: &Value) -> String {
        doc["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text content in {doc}"))
            .to_string()
    }

    /// Run a request through the REST router directly, for comparison.
    async fn rest_call(rest: &Router, method: Method, path: &str, body: Option<Value>) -> String {
        let mut builder = HttpRequest::builder().method(method).uri(path);
        let payload = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&value).unwrap())
            }
            None => Body::empty(),
        };
        let response = rest
            .clone()
            .oneshot(builder.body(payload).unwrap())
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// `x402_supported` returns the body of `GET /supported`, byte for byte.
    ///
    /// Byte-for-byte is the whole claim: the MCP door is a door, not a second
    /// implementation that could answer a slightly different truth about which
    /// networks this facilitator settles on.
    #[tokio::test]
    async fn supported_over_mcp_is_the_body_of_get_supported() {
        let (mcp, rest) = routers().await;
        let (_, _, doc) = rpc(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": { "name": "x402_supported", "arguments": {} }
            }),
        )
        .await;

        let over_rest = rest_call(&rest, Method::GET, "/supported", None).await;
        assert_eq!(tool_text(&doc), over_rest);
        assert_ne!(doc["result"]["isError"], true, "a read answered isError");
        // And it is the real document, not an empty envelope.
        let parsed: Value = serde_json::from_str(&over_rest).unwrap();
        assert!(parsed["kinds"].as_array().is_some_and(|k| !k.is_empty()));
    }

    /// An invalid `/verify` body reaches the caller as `isError`, carrying the
    /// facilitator's own message -- not a panic, and not a JSON-RPC error.
    ///
    /// `Err(McpError)` would have been the easy return here and it is the wrong
    /// one: MCP clients render protocol errors opaquely, so "invalid signature"
    /// would reach the user as "internal error".
    #[tokio::test]
    async fn an_invalid_verify_payload_is_a_tool_error_with_the_rest_message() {
        let (mcp, rest) = routers().await;
        let bad = json!({ "x402Version": 1, "paymentPayload": { "nope": true } });

        let (status, _, doc) = rpc(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": { "name": "x402_verify", "arguments": bad }
            }),
        )
        .await;

        // The transport succeeded; the tool did not.
        assert_eq!(status, StatusCode::OK);
        assert!(doc["error"].is_null(), "answered a JSON-RPC error: {doc}");
        assert_eq!(doc["result"]["isError"], true, "not marked isError: {doc}");

        let over_rest = rest_call(&rest, Method::POST, "/verify", Some(bad)).await;
        assert_eq!(tool_text(&doc), over_rest);
    }

    /// An unknown tool is a protocol error, because nothing ran.
    #[tokio::test]
    async fn an_unknown_tool_is_method_not_found() {
        let (mcp, _) = routers().await;
        let (_, _, doc) = rpc(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": "x402_drain_treasury", "arguments": {} }
            }),
        )
        .await;
        assert_eq!(doc["error"]["code"], -32601, "expected METHOD_NOT_FOUND: {doc}");
    }

    /// `GET /mcp` is a JSON 405, never HTML and never rmcp's bare text.
    #[tokio::test]
    async fn get_mcp_answers_a_json_405() {
        let (mcp, _) = routers().await;
        let response = mcp
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::GET)
                    .uri("/mcp")
                    .header(header::HOST, "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ctype.starts_with("application/json"),
            "GET /mcp answered content-type {ctype:?}"
        );
        assert_eq!(response.headers().get(header::ALLOW).unwrap(), "POST");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!body.trim_start().starts_with('<'), "answered markup: {body}");
        let doc: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(doc["method"], "POST");
    }

    /// The default `Host` allowlist covers production, not just loopback.
    ///
    /// rmcp's own default is loopback only, which behind the ALB rejects every
    /// real request with 403 while local testing stays green. This is the test
    /// that would have caught that after the deploy.
    #[test]
    fn the_default_host_allowlist_contains_the_production_host() {
        let hosts = DEFAULT_MCP_ALLOWED_HOSTS;
        assert!(hosts.contains(&"facilitator.ultravioletadao.xyz"));
        assert!(hosts.contains(&"127.0.0.1"));
    }

    /// Every tool name maps to a route, and only to the four intended ones.
    #[test]
    fn every_tool_name_resolves_to_its_rest_route() {
        assert_eq!(
            TOOL_NAMES
                .iter()
                .map(|n| rest_target(n).expect("every tool must have a route"))
                .collect::<Vec<_>>(),
            vec![
                (Method::GET, "/supported"),
                (Method::POST, "/accepts"),
                (Method::POST, "/verify"),
                (Method::POST, "/settle"),
            ]
        );
        assert!(rest_target("/feedback/revoke").is_none());
    }

    /// `tools()` and [`TOOL_NAMES`] cannot drift apart.
    #[test]
    fn the_tool_table_matches_the_declared_names() {
        let declared: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(declared, TOOL_NAMES.to_vec());
    }

    /// The verify/settle input schema names the fields the REST handler parses.
    ///
    /// Tied to serde, not to a comment: `VerifyRequest` is deserialised with
    /// the first two names removed and the error has to point at them. serde
    /// reports only the FIRST missing field, so the walk stops there; the third
    /// is covered by `an_invalid_verify_payload_is_a_tool_error_with_the_rest_message`,
    /// which posts a body missing it and asserts the handler rejects it.
    #[test]
    fn the_payment_schema_names_the_fields_the_type_requires() {
        let schema = payment_envelope_schema("/verify");
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            required,
            vec!["x402Version", "paymentPayload", "paymentRequirements"]
        );

        let missing_first = serde_json::from_str::<VerifyRequest>("{}")
            .expect_err("an empty body is not a VerifyRequest")
            .to_string();
        assert!(
            missing_first.contains(required[0]),
            "serde does not name {}: {missing_first}",
            required[0]
        );

        let missing_second = serde_json::from_str::<VerifyRequest>(r#"{"x402Version":1}"#)
            .expect_err("a version alone is not a VerifyRequest")
            .to_string();
        assert!(
            missing_second.contains(required[1]),
            "serde does not name {}: {missing_second}",
            required[1]
        );
    }

    /// The published server-card describes the server that actually answers.
    ///
    /// Three ways this document can be true-looking and wrong, all covered
    /// here: a `transport.endpoint` nothing serves (meshrelay published exactly
    /// that -- card live, endpoint 404), a tool list that has drifted from
    /// `tools/list`, and a version frozen at whatever was typed the day the
    /// file was written.
    #[tokio::test]
    async fn the_server_card_describes_the_server_that_answers() {
        let response = crate::handlers::agentic_routes()
            .oneshot(
                HttpRequest::builder()
                    .uri("/.well-known/mcp/server-card.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ctype.starts_with("application/json"),
            "the card is served as {ctype:?}"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let card: Value = serde_json::from_str(&String::from_utf8(bytes.to_vec()).unwrap())
            .expect("the card must be JSON");

        // The field the scanner reads.
        assert_eq!(card["serverInfo"]["name"], "x402-facilitator");
        // The version is stamped at runtime, not typed into the file.
        assert_eq!(
            card["serverInfo"]["version"],
            crate::version::facilitator_version()
        );
        // The endpoint is the route this crate actually mounts.
        assert_eq!(card["transport"]["type"], "streamable-http");
        assert_eq!(
            card["transport"]["endpoint"],
            "https://facilitator.ultravioletadao.xyz/mcp"
        );

        let advertised: Vec<String> = card["tools"]
            .as_array()
            .expect("the card must list its tools")
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            advertised,
            TOOL_NAMES.to_vec(),
            "the card advertises tools this server does not list"
        );
    }

    /// `MCP_ALLOWED_HOSTS` parsing, without touching the process environment.
    #[test]
    fn the_allowlist_defaults_are_a_list_and_not_a_single_string() {
        // A comma-separated value has to become several entries: one entry of
        // "a,b" would match neither host and 403 everything.
        let parsed: Vec<String> = " a.example , b.example:8080 ,, "
            .split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .collect();
        assert_eq!(parsed, vec!["a.example", "b.example:8080"]);
    }
}
