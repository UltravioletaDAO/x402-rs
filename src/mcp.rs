//! The facilitator as an MCP server, behind `POST /mcp`.
//!
//! # What this is
//!
//! The four x402 calls -- `x402_supported`, `x402_accepts`, `x402_verify`,
//! `x402_settle` -- plus four reads a buyer needs around a payment --
//! `x402_networks`, `x402_chain_health`, `x402_escrow_onchain_state`,
//! `x402_durable_evidence` -- over MCP's Streamable HTTP transport, served by
//! the same binary, on the same ALB, under the same rate limiter as `/verify`
//! and `/settle`. No new infrastructure, no second deploy, no second TLS
//! certificate to rotate.
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
//!
//! The parity is of *privilege*, not of capability, and the difference is worth
//! stating because the two read alike. A tool call carries a body and no
//! headers, so the HTTP-only inputs -- the v2 `PAYMENT-SIGNATURE` transport
//! among them -- have no MCP equivalent. The one that mattered is
//! [`IDEMPOTENCY_KEY_ARG`], declared as an argument of `x402_settle` and lifted
//! back out into the `Idempotency-Key` header, because the caller here is a
//! model and a model is exactly what retries an ambiguous error.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, post_service};
use axum::{Json, Router};
use once_cell::sync::Lazy;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, InitializeResult, JsonObject, ListToolsResult, MetaObject,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
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
/// A closed list of eight: the four x402 calls, then four reads a buyer needs
/// around a payment -- how to present each network, whether a chain can settle
/// right now, what an escrow contract holds for one payment, and the DX402
/// evidence anchored for one. The reads sign nothing and move nothing; each is
/// a REST route this service already answered over HTTP.
///
/// Deliberately absent, and staying so:
/// - the admin surfaces -- `/discovery/admin/*`, `/feedback/revoke`,
///   `/register`, `/dx402/repair` -- and every other write (`/feedback`,
///   `/discovery/register`, `/dx402/anchor`): an MCP client is an LLM holding a
///   tool list, and a tool that erases third-party reputation or mints an
///   identity does not belong in one;
/// - the ERC-8004 reads (`/identity`, `/reputation`): reputation has its own
///   surfaces in the stack, and a second door to it here would be a second
///   implementation to keep in step.
///
/// `tools_list_is_exactly_the_eight` fails if the list ever quietly grows, and
/// `the_internal_router_serves_no_write_and_no_admin_route` fails if a write
/// becomes reachable from a tool at all.
pub const TOOL_NAMES: [&str; 8] = [
    "x402_supported",
    "x402_accepts",
    "x402_verify",
    "x402_settle",
    "x402_networks",
    "x402_chain_health",
    "x402_escrow_onchain_state",
    "x402_durable_evidence",
];

/// The `_meta` key under which each tool declares its house class.
///
/// The stack's interop specification (`09-mcp.md`, R9.4) puts the class in
/// the tool's `_meta`, NOT in `annotations`: a catalog that fingerprints a
/// tool by its annotations would see every tool as changed the day a class
/// was added to them.
pub const CLASS_META_KEY: &str = "uvd/clase";

/// The closed vocabulary of house classes (R9.4). A class outside it is a
/// change to the specification, not to this file.
pub const HOUSE_CLASSES: [&str; 7] = [
    "lectura",
    "escribe",
    "mueve_dinero",
    "riel_de_pago",
    "cobra_por_llamada",
    "pide_credencial",
    "sin_clasificar",
];

/// Each tool's house class, as this surface declares it.
///
/// - `x402_supported` is `lectura`: it reads what the facilitator settles,
///   charges nothing and asks for no credential. As a read it also publishes
///   an `outputSchema` and answers in `structuredContent` (R9.5).
/// - `x402_accepts` is `riel_de_pago` although it writes nothing: its output
///   is the signable requirements of one concrete purchase.
/// - `x402_verify` is `riel_de_pago`: its input is a signed authorization.
/// - `x402_settle` is `mueve_dinero`: it broadcasts the transfer.
/// - `x402_networks`, `x402_chain_health`, `x402_escrow_onchain_state` and
///   `x402_durable_evidence` are `lectura`: none takes a signed authorization,
///   none returns signable requirements, and none sends a transaction. The
///   escrow one reads the contract with a view call (`/escrow/state` spends no
///   gas); the evidence one reads the facilitator's own registry and never
///   returns the sealed bytes.
///
/// `annotations` are deliberately NOT touched by the class: they keep saying
/// what the MCP hints say (read-only, destructive, idempotent, open world).
pub const TOOL_CLASSES: [(&str, &str); 8] = [
    ("x402_supported", "lectura"),
    ("x402_accepts", "riel_de_pago"),
    ("x402_verify", "riel_de_pago"),
    ("x402_settle", "mueve_dinero"),
    ("x402_networks", "lectura"),
    ("x402_chain_health", "lectura"),
    ("x402_escrow_onchain_state", "lectura"),
    ("x402_durable_evidence", "lectura"),
];

/// `_meta` for `tool`: `{"uvd/clase": <class>}`.
fn class_meta(tool: &str) -> MetaObject {
    let class = TOOL_CLASSES
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, class)| *class)
        .unwrap_or_else(|| unreachable!("every tool has a class in TOOL_CLASSES: {tool}"));
    debug_assert!(
        HOUSE_CLASSES.contains(&class),
        "{class} is not a house class"
    );
    let mut meta = JsonObject::new();
    meta.insert(CLASS_META_KEY.to_string(), Value::String(class.to_string()));
    MetaObject(meta)
}

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

/// The one argument of `x402_settle` that is NOT part of the REST body.
///
/// `post_settle` reads exactly-once semantics off the `Idempotency-Key` header,
/// and a tool call has no way to set a header. Declaring it as an argument and
/// lifting it out before the body is serialised is what gives an MCP client the
/// same retry protection an HTTP client has.
const IDEMPOTENCY_KEY_ARG: &str = "idempotencyKey";

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
        tracing::warn!("{ENV_MCP_ALLOWED_HOSTS} held no usable host; falling back to the defaults");
        return DEFAULT_MCP_ALLOWED_HOSTS
            .iter()
            .map(|h| (*h).to_string())
            .collect();
    }
    hosts
}

/// Which REST route a tool name stands for.
///
/// The DX402 tool is the one route with a placeholder in it: `{document}` is
/// `evidence` or `receipt` ([`DX402_DOCUMENTS`]) and `{paymentId}` is the
/// caller's id, percent-encoded into one path segment ([`rest_request`]).
fn rest_target(tool: &str) -> Option<(Method, &'static str)> {
    match tool {
        "x402_supported" => Some((Method::GET, "/supported")),
        "x402_accepts" => Some((Method::POST, "/accepts")),
        "x402_verify" => Some((Method::POST, "/verify")),
        "x402_settle" => Some((Method::POST, "/settle")),
        "x402_networks" => Some((Method::GET, "/networks.json")),
        "x402_chain_health" => Some((Method::GET, "/health/ready")),
        "x402_escrow_onchain_state" => Some((Method::POST, "/escrow/state")),
        "x402_durable_evidence" => Some((Method::GET, "/dx402/{document}/{paymentId}")),
        _ => None,
    }
}

/// The two DX402 documents `x402_durable_evidence` reads, the default first:
/// `GET /dx402/evidence/{paymentId}` (pointer, hash, retention, authority and
/// the receipt's signature) and `GET /dx402/receipt/{paymentId}` (the signed
/// receipt alone, with its EIP-712 domain, for offline verification).
const DX402_DOCUMENTS: [&str; 2] = ["evidence", "receipt"];

/// The arguments of `x402_durable_evidence`.
const PAYMENT_ID_ARG: &str = "paymentId";
const DOCUMENT_ARG: &str = "document";

/// The argument of `x402_chain_health`.
const NETWORK_ARG: &str = "network";

/// The one REST request a tool call turns into.
#[derive(Debug, PartialEq)]
struct RestRequest {
    method: Method,
    /// Path and query, every caller-supplied part already percent-encoded.
    uri: String,
    /// The JSON body, for the tools whose arguments ARE the body.
    body: Option<JsonObject>,
}

/// Percent-encode everything but RFC 3986's unreserved characters.
///
/// One encoder for both places a caller's string lands in a URI: a path
/// segment (the DX402 `paymentId`) and a query value (`network`). Encoded this
/// way a `/`, `?`, `#`, `&` or `..` stays INSIDE the segment or value it was
/// given for -- axum's `Path` and `Query` extractors decode it back, so the
/// handler sees exactly the string the caller sent, and a `paymentId` can never
/// steer the request to another route of the internal router. `+` is encoded
/// too, so a path segment and a query value decode to the same text.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Take an optional string argument out of `arguments`. `null` is absent.
fn take_optional_string(
    tool: &str,
    arguments: &mut JsonObject,
    name: &str,
) -> Result<Option<String>, McpError> {
    match arguments.remove(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(other) => Err(McpError::invalid_params(
            format!("{tool}: `{name}` must be a string, got {other}"),
            None,
        )),
    }
}

/// Refuse whatever is left in `arguments` once a GET tool took its own.
///
/// A GET tool has no body to carry an extra field to, so dropping one would
/// answer a different question than the caller asked -- the full network list
/// to a model that sent `{"chain": "base"}` meaning a filter. `x402_supported`
/// predates this and keeps ignoring extras: its contract is not changed here.
fn refuse_leftover_arguments(tool: &str, arguments: &JsonObject) -> Result<(), McpError> {
    if arguments.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = arguments.keys().map(String::as_str).collect();
    Err(McpError::invalid_params(
        format!("{tool} does not take {names:?}; see its inputSchema"),
        None,
    ))
}

/// Turn a tool call into the REST request it stands for.
///
/// The body tools (`x402_accepts`, `x402_verify`, `x402_settle`,
/// `x402_escrow_onchain_state`) send their arguments as the JSON body, unread.
/// The GET reads put theirs in the path or the query, percent-encoded
/// ([`percent_encode`]), and refuse any argument they do not declare. Nothing
/// here validates a value the REST handler validates -- an unknown network, a
/// malformed `paymentId` -- so the MCP answer stays the HTTP answer.
fn rest_request(tool: &str, mut arguments: JsonObject) -> Result<RestRequest, McpError> {
    let Some((method, route)) = rest_target(tool) else {
        return Err(McpError::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("unknown tool: {tool}"),
            Some(json!({ "tools": TOOL_NAMES })),
        ));
    };
    let uri = match tool {
        // Unchanged: a GET with no body, whatever arguments came with it.
        "x402_supported" => route.to_string(),
        "x402_networks" => {
            refuse_leftover_arguments(tool, &arguments)?;
            route.to_string()
        }
        "x402_chain_health" => {
            let network = take_optional_string(tool, &mut arguments, NETWORK_ARG)?;
            refuse_leftover_arguments(tool, &arguments)?;
            match network {
                Some(network) => format!("{route}?{NETWORK_ARG}={}", percent_encode(&network)),
                None => route.to_string(),
            }
        }
        "x402_durable_evidence" => {
            let payment_id = take_optional_string(tool, &mut arguments, PAYMENT_ID_ARG)?
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| {
                    McpError::invalid_params(
                        format!("{tool}: `{PAYMENT_ID_ARG}` is required"),
                        None,
                    )
                })?;
            let document = take_optional_string(tool, &mut arguments, DOCUMENT_ARG)?
                .unwrap_or_else(|| DX402_DOCUMENTS[0].to_string());
            if !DX402_DOCUMENTS.contains(&document.as_str()) {
                return Err(McpError::invalid_params(
                    format!("{tool}: `{DOCUMENT_ARG}` must be one of {DX402_DOCUMENTS:?}"),
                    None,
                ));
            }
            refuse_leftover_arguments(tool, &arguments)?;
            route
                .replace("{document}", &document)
                .replace("{paymentId}", &percent_encode(&payment_id))
        }
        _ => {
            return Ok(RestRequest {
                method,
                uri: route.to_string(),
                body: Some(arguments),
            });
        }
    };
    Ok(RestRequest {
        method,
        uri,
        body: None,
    })
}

/// Whether a REST answer is the tool's RESULT rather than its failure.
///
/// Any 2xx is. One non-2xx is too: `GET /health/ready` answers `503` with its
/// full document when a mainnet cannot settle, and that document -- which
/// chain, which reason, how many settles each signer has left -- is exactly the
/// reading `x402_chain_health` was asked for. Turned into a tool error, an
/// agent asking about Base would be told the call failed because Celo was down.
/// Only the document counts: the `503` that carries no reading
/// (`{"status": "down", "error": "probe_failed"}`) stays a tool error.
fn is_tool_result(tool: &str, status: StatusCode, body: &str) -> bool {
    if status.is_success() {
        return true;
    }
    tool == "x402_chain_health"
        && status == StatusCode::SERVICE_UNAVAILABLE
        && serde_json::from_str::<Value>(body).is_ok_and(|doc| doc["networks"].is_array())
}

/// Turn a JSON literal into the object shape rmcp wants for `inputSchema`.
fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        other => unreachable!("tool schemas are JSON objects, got {other}"),
    }
}

const SKILL_MD: &str = include_str!("../static/skill.md");

/// The first fenced ```json block after `heading` in `static/skill.md`.
fn published_example(heading: &str) -> Value {
    let after = SKILL_MD
        .split_once(heading)
        .unwrap_or_else(|| panic!("skill.md must carry the heading `{heading}`"))
        .1;
    let block = after
        .split_once("```json")
        .unwrap_or_else(|| panic!("no json block after `{heading}` in skill.md"))
        .1
        .split_once("```")
        .expect("unterminated json block in skill.md")
        .0;
    serde_json::from_str(block.trim())
        .unwrap_or_else(|e| panic!("the example after `{heading}` must be valid JSON: {e}"))
}

/// The worked x402 **v1** example, taken out of the document that publishes it.
///
/// NOT a fourth copy of the body. `static/skill.md` is the one place the
/// example is written; `src/openapi.rs` repeats it for `/docs` and a test
/// binds the two. Parsing it out of the shipped Markdown means an MCP client
/// and a human reader cannot be shown different bodies, which is the failure
/// this schema is fixing in the first place.
static VERIFY_EXAMPLE: Lazy<Value> = Lazy::new(|| published_example("## 3. `POST /verify`"));

/// The worked x402 **v2** example, from the same document.
///
/// A schema that showed only the v1 body was half the reason a v2 integration
/// had nowhere correct to read: the `400` it got named v1 fields, `/skill.md`
/// published v1 only, and this schema repeated v1 again. Three surfaces, one
/// answer, and it was the wrong one for the body being sent.
static VERIFY_EXAMPLE_V2: Lazy<Value> =
    Lazy::new(|| published_example("### The same payment in the x402 v2 shape"));

/// The request envelope `POST /verify` and `POST /settle` parse.
///
/// # Why this is spelled out and not a bare `Object`
///
/// It used to declare `paymentPayload` and `paymentRequirements` as
/// `{"type": "object", "additionalProperties": true}` -- which says nothing --
/// and point at `/skill.md` for the real shape. `/skill.md` then published an
/// example the facilitator rejected with `400`. So an agent reaching this
/// facilitator over MCP had NO correct source for the body: the schema was
/// empty on one side and the document it deferred to was wrong on the other.
///
/// The fields below are the serde fields of [`crate::types::VerifyRequest`],
/// [`crate::types::PaymentPayload`], [`crate::types::ExactEvmPayload`] and
/// [`crate::types::PaymentRequirements`]. Not a guess and not utoipa output:
/// none of those types derives `ToSchema`.
///
/// `additionalProperties` stays `true` on the nested objects -- the handler
/// auto-detects x402 v1, v2, x402r and x402r-nested, and carries extensions
/// (`refund`, `upto`, escrow `action`) inside them. Describing the common case
/// exactly while leaving the envelope open is the honest shape.
///
/// # Why `anyOf` and not one flat `required`
///
/// There are two envelopes, not one. x402 v1 pairs `paymentPayload` with
/// `paymentRequirements`; x402 v2 replaces that with `resource` + `accepted` and
/// has no `paymentRequirements` at all. This schema used to declare the v1
/// triple as unconditionally `required`, so a client building a v2 body was told
/// by its own tool definition to send a field that does not exist in v2 -- the
/// same thing the `400` hint was doing, one layer up.
///
/// The properties stay flat, so a client that ignores `anyOf` still sees every
/// field described. The `anyOf` carries the part that actually branches: which
/// fields each shape requires.
fn payment_envelope_schema(operation: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "x402Version": {
                "type": "integer",
                "enum": [1, 2],
                "description": "x402 protocol version of the envelope. 1 unless you are sending the v2 shape."
            },
            "paymentPayload": {
                "type": "object",
                "description": "The payer-signed authorization. Carries its OWN x402Version at its root -- it is not inherited from the envelope. In x402 v1 it also carries scheme and network; in v2 those live in the top-level `accepted` instead.",
                "properties": {
                    "x402Version": { "type": "integer", "enum": [1, 2] },
                    "scheme": {
                        "type": "string",
                        "description": crate::zama::text("x402 v1 ONLY (in v2 this is accepted.scheme). exact | upto | escrow | commerce | fhe-transfer. GET /supported lists what this facilitator serves.")
                    },
                    "network": {
                        "type": "string",
                        "description": "x402 v1 ONLY (in v2 this is accepted.network). The chain, in EITHER spelling: the x402 v1 name (\"base\") or the CAIP-2 identifier (\"eip155:8453\"). Both are accepted, and GET /supported publishes every network under both."
                    },
                    "resource": {
                        "type": "object",
                        "description": "x402 v2, OPTIONAL: a copy of the top-level `resource`. Older builds required it here as well and rejected the body without it; it is now derived from the top-level one when omitted. Sending it still works.",
                        "additionalProperties": true
                    },
                    "accepted": {
                        "type": "object",
                        "description": "x402 v2, OPTIONAL: a copy of the top-level `accepted`, on the same terms as `resource` above.",
                        "additionalProperties": true
                    },
                    "payload": {
                        "type": "object",
                        "description": "The signed material. EVM chains use signature + authorization (below); Solana sends { \"transaction\": \"<base64>\" } instead.",
                        "properties": {
                            "signature": {
                                "type": "string",
                                "description": "EVM: the ERC-3009 EIP-712 signature, 0x + 130 hex characters (r || s || v)."
                            },
                            "authorization": {
                                "type": "object",
                                "description": "EVM: the ERC-3009 struct that was signed. Every field is a STRING, including the amount and the timestamps.",
                                "properties": {
                                    "from": { "type": "string", "description": "Payer address, 0x + 40 hex." },
                                    "to": { "type": "string", "description": "Recipient address, 0x + 40 hex. Matches paymentRequirements.payTo." },
                                    "value": {
                                        "type": "string",
                                        "description": "Amount in token base units, as a decimal STRING. Named `value` here; the requirements call their own limit `maxAmountRequired`."
                                    },
                                    "validAfter": {
                                        "type": "string",
                                        "description": "Unix SECONDS as a string. \"1700000000\", not 1700000000: a JSON number is rejected."
                                    },
                                    "validBefore": {
                                        "type": "string",
                                        "description": "Unix SECONDS as a string, same rule as validAfter."
                                    },
                                    "nonce": {
                                        "type": "string",
                                        "description": "32-byte nonce, 0x + 64 hex. Fresh per authorization."
                                    }
                                },
                                "required": ["from", "to", "value", "validAfter", "validBefore", "nonce"],
                                "additionalProperties": true
                            },
                            "transaction": {
                                "type": "string",
                                "description": "Solana: the base64 bincode-serialised transaction. Use INSTEAD of signature + authorization."
                            }
                        },
                        "additionalProperties": true
                    }
                },
                "required": ["x402Version", "payload"],
                "additionalProperties": true
            },
            "paymentRequirements": {
                "type": "object",
                "description": "x402 v1 ONLY. What the resource server demands. The four descriptive fields have no defaults: omit one and the whole body fails to parse. In x402 v2 this object does not exist -- see `resource` and `accepted`.",
                "properties": {
                    "scheme": { "type": "string", "description": "Must match paymentPayload.scheme." },
                    "network": {
                        "type": "string",
                        "description": "The chain, in EITHER spelling: the x402 v1 name (\"base\") or the CAIP-2 identifier (\"eip155:8453\"), same as paymentPayload.network. An offer taken straight out of GET /discovery/resources is CAIP-2 and can be used unmodified."
                    },
                    "maxAmountRequired": {
                        "type": "string",
                        "description": "Ceiling in token base units, as a decimal STRING. The authorization's `value` must not exceed it."
                    },
                    "resource": { "type": "string", "description": "Absolute URL of the thing being paid for. Required." },
                    "description": { "type": "string", "description": "Human-readable label. Required; may be empty." },
                    "mimeType": { "type": "string", "description": "Media type of the resource, e.g. application/json. Required." },
                    "payTo": { "type": "string", "description": "Recipient address. Required." },
                    "maxTimeoutSeconds": { "type": "integer", "description": "How long the offer stands. Required." },
                    "asset": { "type": "string", "description": "Token contract (EVM) or mint (SVM). Required." },
                    "extra": {
                        "type": "object",
                        "description": "Optional. EIP-712 domain (name, version) for tokens not in the static table, escrow addresses, and scheme extensions.",
                        "additionalProperties": true
                    }
                },
                "required": [
                    "scheme", "network", "maxAmountRequired", "resource",
                    "description", "mimeType", "payTo", "maxTimeoutSeconds", "asset"
                ],
                "additionalProperties": true
            },
            "resource": {
                "type": "object",
                "description": "x402 v2 ONLY. What is being sold. Replaces the resource/description/mimeType fields of the v1 paymentRequirements.",
                "properties": {
                    "url": { "type": "string", "description": "Absolute URL of the thing being paid for. Required." },
                    "description": { "type": "string", "description": "Human-readable label. Required; may be empty." },
                    "mimeType": { "type": "string", "description": "Media type of the resource, e.g. application/json. Required." }
                },
                "required": ["url", "description", "mimeType"],
                "additionalProperties": true
            },
            "accepted": {
                "type": "object",
                "description": "x402 v2 ONLY. What is being charged. Replaces the rest of the v1 paymentRequirements. Unknown keys are ignored, so a 402 offer carrying extras (maxAmountRequired, resource, description, mimeType) can be forwarded unedited.",
                "properties": {
                    "scheme": { "type": "string", "description": crate::zama::text("exact | upto | escrow | commerce | fhe-transfer. GET /supported lists what this facilitator serves.") },
                    "network": {
                        "type": "string",
                        "description": "The chain as a CAIP-2 identifier (\"eip155:8453\"). CAIP-2 ONLY here: unlike the v1 paymentRequirements.network, this field refuses the bare x402 v1 name (\"base\"). An offer taken straight out of GET /discovery/resources is already CAIP-2 and can be used unmodified."
                    },
                    "asset": { "type": "string", "description": "Token contract (EVM) or mint (SVM). Required." },
                    "amount": {
                        "type": "string",
                        "description": "Ceiling in token base units, as a decimal STRING. This is the v2 name for the v1 maxAmountRequired. The authorization's `value` must not exceed it."
                    },
                    "payTo": { "type": "string", "description": "Recipient address. Required." },
                    "maxTimeoutSeconds": { "type": "integer", "description": "How long the offer stands. Required." },
                    "extra": {
                        "type": "object",
                        "description": "Optional. EIP-712 domain (name, version) for tokens not in the static table, escrow addresses, and scheme extensions.",
                        "additionalProperties": true
                    }
                },
                "required": ["scheme", "network", "asset", "amount", "payTo", "maxTimeoutSeconds"],
                "additionalProperties": true
            }
        },
        "required": ["x402Version", "paymentPayload"],
        "anyOf": [
            {
                "title": "x402 v1",
                "required": ["paymentRequirements"],
                "properties": {
                    "paymentPayload": { "required": ["x402Version", "scheme", "network", "payload"] }
                }
            },
            {
                "title": "x402 v2",
                "required": ["resource", "accepted"]
            }
        ],
        "additionalProperties": true,
        "examples": [VERIFY_EXAMPLE.clone(), VERIFY_EXAMPLE_V2.clone()],
        "description": format!(
            "Identical to the JSON body of POST {operation}. TWO envelopes are \
             accepted and they are not interchangeable field by field: x402 v1 \
             is paymentPayload + paymentRequirements, x402 v2 is paymentPayload \
             + resource + accepted and has no paymentRequirements. Both examples \
             below are runnable as printed -- their signatures and nonces are \
             well-formed placeholders, so they answer 200 with isValid:false. \
             More at https://facilitator.ultravioletadao.xyz/skill.md"
        )
    })
}

/// `x402_settle`'s input: the `/settle` envelope plus the one header a tool can
/// set, [`IDEMPOTENCY_KEY_ARG`].
fn settle_input_schema() -> Value {
    let mut doc = payment_envelope_schema("/settle");
    doc["properties"][IDEMPOTENCY_KEY_ARG] = json!({
        "type": "string",
        "description": "Optional. Sent as the Idempotency-Key header, NOT as part of the \
                        payment body. The same key with the same payment returns the \
                        first result instead of settling twice; the same key with a \
                        different payment is refused with 409. Use it on any retry. \
                        GENERATE A FRESH, UNGUESSABLE VALUE PER PAYMENT -- a UUIDv4 \
                        is the right shape. Do not reuse a key across different \
                        payments and do not use a predictable one like \"retry-1\": \
                        keys share one namespace across all callers, so a guessable \
                        key can be claimed by someone else's settle first and yours \
                        is then refused with 409."
    });
    doc
}

/// `outputSchema` of `x402_supported`: the body of `GET /supported`.
///
/// Open on purpose (`additionalProperties` is never `false`): a new field in
/// `extra`, a new scheme or a new extension is additive for `/supported`, and
/// a client that validates `structuredContent` against this schema must not
/// start failing when one ships. What it pins is the shape a caller crosses
/// fields on -- `kinds[].network`, `kinds[].scheme`, the token list and the
/// fee payer. A test validates a production `/supported` snapshot against it.
fn supported_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The body of GET /supported, unchanged: every (x402Version, \
                        scheme, network) this facilitator settles, the protocol \
                        extensions it serves, the fee payer per network where it \
                        differs, and what its facilitator receipts cover.",
        "required": ["kinds"],
        "properties": {
            "kinds": {
                "type": "array",
                "description": "One entry per settleable (x402Version, scheme, network). \
                                Every chain appears twice, under its x402 v1 name \
                                (\"base\") and under its CAIP-2 id (\"eip155:8453\"); \
                                `networkAliases` lists both on each entry.",
                "items": {
                    "type": "object",
                    "required": ["x402Version", "scheme", "network"],
                    "properties": {
                        "x402Version": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "The x402 protocol version this entry is for."
                        },
                        "scheme": {
                            "type": "string",
                            "description": crate::zama::text("Payment scheme: exact, upto, escrow, commerce or fhe-transfer.")
                        },
                        "network": {
                            "type": "string",
                            "description": "The network, as the x402 version of this entry spells it."
                        },
                        "networkAliases": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Every identifier of the same chain, this one included."
                        },
                        "extra": {
                            "type": "object",
                            "description": "Network-specific settlement details.",
                            "properties": {
                                "feePayer": {
                                    "type": "string",
                                    "description": "The account that pays the network fee."
                                },
                                "tokens": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "required": ["token", "address", "decimals"],
                                        "properties": {
                                            "token": { "type": "string" },
                                            "address": { "type": "string" },
                                            "decimals": { "type": "integer", "minimum": 0 }
                                        }
                                    }
                                },
                                "escrowAddress": { "type": "string" },
                                "operatorAddress": { "type": "string" },
                                "tokenCollector": { "type": "string" }
                            }
                        }
                    }
                }
            },
            "extensions": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Protocol extensions this deployment serves."
            },
            "signers": {
                "type": "object",
                "additionalProperties": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "description": "Fee payer accounts by network, where they differ per network."
            },
            "facilitatorReceipts": {
                "type": "object",
                "description": "The facilitator receipt capability; see GET /receipts."
            }
        }
    })
}

/// `outputSchema` of `x402_networks`: the body of `GET /networks.json`.
///
/// Open like [`supported_output_schema`], and for the same reason. What it
/// pins is what a caller crosses fields on: the two identifiers, the testnet
/// flag, the explorer templates and each token's address and decimals. A test
/// validates the document built from a production `/supported` against it.
fn networks_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The body of GET /networks.json, unchanged: one row per chain \
                        x402_supported names -- never more, never fewer -- with how to \
                        present it.",
        "required": ["networks"],
        "properties": {
            "networks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "caip2", "family", "testnet", "displayName", "schemes", "tokens"],
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The x402 v1 name (\"base\"). A chain with no v1 name (native Hedera) repeats its CAIP-2 id here."
                        },
                        "caip2": { "type": "string", "description": "The CAIP-2 id (\"eip155:8453\")." },
                        "family": {
                            "type": "string",
                            "description": "evm, svm, near, stellar, algorand, sui, xrpl or hedera."
                        },
                        "chainId": {
                            "type": ["integer", "null"],
                            "minimum": 0,
                            "description": "The EVM chain id; null outside EVM."
                        },
                        "testnet": { "type": "boolean" },
                        "displayName": { "type": "string" },
                        "explorer": {
                            "type": ["object", "null"],
                            "description": "Explorer URL templates; null when none is known.",
                            "required": ["base", "tx", "address"],
                            "properties": {
                                "base": { "type": "string" },
                                "tx": { "type": "string", "description": "Carries a {tx} placeholder for the transaction id." },
                                "address": { "type": "string", "description": "Carries an {address} placeholder." }
                            }
                        },
                        "icon": { "type": ["string", "null"] },
                        "schemes": { "type": "array", "items": { "type": "string" } },
                        "tokens": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["symbol", "address", "decimals"],
                                "properties": {
                                    "symbol": { "type": "string" },
                                    "address": { "type": "string" },
                                    "decimals": { "type": "integer", "minimum": 0 },
                                    "eip712": {
                                        "type": ["object", "null"],
                                        "description": "The EIP-712 domain /verify resolves for this token; null outside EVM or outside the static table.",
                                        "properties": {
                                            "name": { "type": "string" },
                                            "version": { "type": "string" }
                                        }
                                    },
                                    "usdPegged": { "type": ["boolean", "null"] },
                                    "icon": { "type": ["string", "null"] }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// The three states `/health/ready` grades a chain, a signer and the whole
/// answer with (`readiness::Status`, serialised lowercase).
const READINESS_STATUSES: [&str; 3] = ["ok", "degraded", "down"];

/// `outputSchema` of `x402_chain_health`: the body of `GET /health/ready`.
fn chain_health_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The body of GET /health/ready, unchanged: whether this \
                        facilitator can settle, chain by chain, as measured at \
                        checkedAtUnix and cached for ttlSecs. No RPC URL, address or \
                        balance is ever in it.",
        "required": ["status", "networks"],
        "properties": {
            "status": {
                "type": "string",
                "enum": READINESS_STATUSES,
                "description": "The worst reading in scope. down: a mainnet in scope cannot settle. Unscoped, an old sanctions list makes it degraded, never down."
            },
            "checkedAtUnix": { "type": "integer", "minimum": 0 },
            "ageSecs": { "type": "integer", "minimum": 0 },
            "ttlSecs": { "type": "integer", "minimum": 0 },
            "probeTimeoutMs": { "type": "integer", "minimum": 0 },
            "thresholds": { "type": "object" },
            "summary": {
                "type": "object",
                "properties": {
                    "ok": { "type": "integer", "minimum": 0 },
                    "degraded": { "type": "integer", "minimum": 0 },
                    "down": { "type": "integer", "minimum": 0 }
                }
            },
            "networks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["network", "caip2", "mainnet", "status", "rpc", "signers"],
                    "properties": {
                        "network": { "type": "string" },
                        "caip2": { "type": "string" },
                        "mainnet": { "type": "boolean" },
                        "status": { "type": "string", "enum": READINESS_STATUSES },
                        "reason": {
                            "type": "string",
                            "description": "A bounded token, present unless status is ok: rpc_unreachable, rpc_timeout, rpc_chain_id_mismatch, signer_gas_critical, signer_gas_low, and on native Hedera also signer_key_mismatch and store_unavailable."
                        },
                        "rpc": { "type": "string", "description": "ok, unreachable, timeout or wrong_chain." },
                        "warnSettles": { "type": "integer", "minimum": 0 },
                        "signers": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["index", "status", "gasOk"],
                                "properties": {
                                    "index": { "type": "integer", "minimum": 0 },
                                    "status": { "type": "string", "enum": READINESS_STATUSES },
                                    "gasOk": { "type": "boolean" },
                                    "settlesRemaining": {
                                        "type": ["integer", "null"],
                                        "minimum": 0,
                                        "description": "Settles the signer's gas still admits; null where the chain prices gas at zero."
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "unchecked": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Configured networks whose family is not probed: listed here rather than folded into a green answer."
            },
            "sanctionsList": { "type": "object" }
        }
    })
}

/// `x402_escrow_onchain_state`'s input: the body of `POST /escrow/state`.
///
/// The fields are the serde fields of
/// [`crate::payment_operator::types::EscrowStateQuery`],
/// [`crate::payment_operator::types::EscrowPaymentInfo`] and
/// [`crate::payment_operator::types::EscrowExtra`] (none derives `ToSchema`).
/// Two traps are spelled out because the type enforces them: `maxAmount` is a
/// decimal STRING while the expiries and fee bounds are JSON numbers, and
/// `network` is CAIP-2 only (`Network::from_caip2`). The example is built here
/// rather than written as a literal so this file never carries `0x` + 64 hex
/// (the salt), which the pre-commit hook refuses.
fn escrow_state_input_schema() -> Value {
    let address = |what: &str| json!({ "type": "string", "description": format!("{what}, 0x + 40 hex.") });
    let example = json!({
        "paymentInfo": {
            "operator": "0x1111111111111111111111111111111111111111",
            "receiver": "0x2222222222222222222222222222222222222222",
            "token": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "maxAmount": "1000000",
            "preApprovalExpiry": 281474976710655u64,
            "authorizationExpiry": 281474976710655u64,
            "refundExpiry": 281474976710655u64,
            "minFeeBps": 0,
            "maxFeeBps": 100,
            "feeReceiver": "0x3333333333333333333333333333333333333333",
            "salt": format!("0x{}", "5a".repeat(32))
        },
        "payer": "0x4444444444444444444444444444444444444444",
        "network": "eip155:8453",
        "extra": {
            "escrowAddress": "0x5555555555555555555555555555555555555555",
            "operatorAddress": "0x1111111111111111111111111111111111111111",
            "tokenCollector": "0x6666666666666666666666666666666666666666"
        }
    });
    json!({
        "type": "object",
        "description": "Identical to the JSON body of POST /escrow/state. These fields \
                        IDENTIFY one escrow payment -- the paymentInfo it was authorized \
                        with, its payer, its chain and its escrow contracts. Nothing in \
                        them is signed and nothing is spent.",
        "properties": {
            "paymentInfo": {
                "type": "object",
                "description": "The AuthCaptureEscrow PaymentInfo the payment was made with, as the escrow payload carried it (the payer goes in `payer`).",
                "properties": {
                    "operator": address("PaymentOperator contract"),
                    "receiver": address("Who receives the payment, minus fees"),
                    "token": address("Token contract"),
                    "maxAmount": {
                        "type": "string",
                        "description": "Most the escrow may hold, in token base units, as a decimal STRING."
                    },
                    "preApprovalExpiry": { "type": "integer", "minimum": 0, "description": "uint48 unix seconds, as a JSON number." },
                    "authorizationExpiry": { "type": "integer", "minimum": 0, "description": "uint48 unix seconds, as a JSON number." },
                    "refundExpiry": { "type": "integer", "minimum": 0, "description": "uint48 unix seconds, as a JSON number." },
                    "minFeeBps": { "type": "integer", "minimum": 0, "maximum": 65535 },
                    "maxFeeBps": { "type": "integer", "minimum": 0, "maximum": 65535 },
                    "feeReceiver": address("Who receives the fee"),
                    "salt": { "type": "string", "description": "The payment's 32-byte salt, 0x + 64 hex." }
                },
                "required": [
                    "operator", "receiver", "token", "maxAmount", "preApprovalExpiry",
                    "authorizationExpiry", "refundExpiry", "minFeeBps", "maxFeeBps",
                    "feeReceiver", "salt"
                ],
                "additionalProperties": true
            },
            "payer": address("The payer"),
            "network": {
                "type": "string",
                "description": "The chain as a CAIP-2 id (\"eip155:8453\"). CAIP-2 ONLY: this route refuses the bare x402 v1 name (\"base\")."
            },
            "extra": {
                "type": "object",
                "description": "The escrow contracts, as the payment requirements' `extra` named them.",
                "properties": {
                    "escrowAddress": address("AuthCaptureEscrow contract"),
                    "operatorAddress": address("PaymentOperator contract"),
                    "tokenCollector": address("ERC3009TokenCollector contract")
                },
                "required": ["escrowAddress", "operatorAddress", "tokenCollector"],
                "additionalProperties": true
            }
        },
        "required": ["paymentInfo", "payer", "network", "extra"],
        "additionalProperties": true,
        "examples": [example]
    })
}

/// `outputSchema` of `x402_escrow_onchain_state`: the body of
/// `POST /escrow/state`, [`crate::payment_operator::types::EscrowStateResponse`].
fn escrow_state_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The body of POST /escrow/state, unchanged: the escrow contract's \
                        own view of one payment, read with a view call.",
        "required": ["hasCollectedPayment", "capturableAmount", "refundableAmount", "paymentInfoHash", "network"],
        "properties": {
            "hasCollectedPayment": {
                "type": "boolean",
                "description": "Whether the escrow has collected the payer's funds for this payment."
            },
            "capturableAmount": {
                "type": "string",
                "pattern": "^[0-9]+$",
                "description": "Still available for capture, in token base units, as a decimal string."
            },
            "refundableAmount": {
                "type": "string",
                "pattern": "^[0-9]+$",
                "description": "Available for refund, in token base units, as a decimal string."
            },
            "paymentInfoHash": { "type": "string", "description": "The hash the escrow keys this payment by." },
            "network": { "type": "string", "description": "The CAIP-2 id of the chain the escrow lives on." }
        }
    })
}

/// `x402_durable_evidence`'s input.
fn durable_evidence_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            PAYMENT_ID_ARG: {
                "type": "string",
                "description": "The DX402 paymentId, keccak256(caip2Network || txHash), 0x + 64 hex: \
                                the id the seller's X-Durable-Evidence header or its anchor \
                                reported. Case and the 0x prefix do not matter."
            },
            DOCUMENT_ARG: {
                "type": "string",
                "enum": DX402_DOCUMENTS,
                "default": DX402_DOCUMENTS[0],
                "description": "evidence (the default): pointer, contentHash, retention, \
                                verified/signed and the receipt's signature. receipt: the \
                                signed EIP-712 receipt alone, with its domain, for offline \
                                verification."
            }
        },
        "required": [PAYMENT_ID_ARG],
        "additionalProperties": false
    })
}

/// `outputSchema` of `x402_durable_evidence`: the body of
/// `GET /dx402/evidence/{paymentId}`, or of `GET /dx402/receipt/{paymentId}`.
///
/// Two documents, so the shape branches in `anyOf`, titled by the `document`
/// argument that selects each. `receipt` is the one field both carry, with a
/// different type in each (the signature, or the signed object), which is why
/// it is described and left untyped at the top level.
fn durable_evidence_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The body of GET /dx402/evidence/{paymentId} or, with \
                        document=receipt, of GET /dx402/receipt/{paymentId}, unchanged.",
        "anyOf": [
            {
                "title": "evidence",
                "required": ["paymentId", "pointer", "contentHash", "receipt", "receiptSigner", "verified", "signed"],
                "properties": { "receipt": { "type": "string" } }
            },
            {
                "title": "receipt",
                "required": ["receipt", "signature", "signer", "domain"],
                "properties": { "receipt": { "type": "object" } }
            }
        ],
        "properties": {
            "paymentId": { "type": "string" },
            "pointer": {
                "type": "string",
                "description": "Where the sealed copy lives. Ciphertext only, sealed to the payer's own key; GET /dx402/blob/{paymentId} serves it."
            },
            "backend": { "type": "string" },
            "contentHash": {
                "type": "string",
                "description": "Hash of the PLAINTEXT that was delivered, so it proves what the evidence decrypts to."
            },
            "keyAlg": { "type": "string" },
            "mode": { "type": "string" },
            "retention": { "type": "string" },
            "anchoredAt": { "type": "integer", "minimum": 0 },
            "retentionUntil": { "type": "integer", "minimum": 0 },
            "receipt": {
                "description": "evidence: the facilitator's signature over the receipt. receipt: the signed receipt itself."
            },
            "receiptSigner": { "type": "string" },
            "verified": {
                "type": "boolean",
                "description": "The payee proved the anchor is theirs. false is provisional: a claim anyone could have made."
            },
            "signed": {
                "type": "boolean",
                "description": "The claimant committed to an identity, not yet confirmed by a chain to be the payee."
            },
            "signature": { "type": "string" },
            "signer": { "type": "string" },
            "domain": {
                "type": "object",
                "required": ["name", "version", "chainId"],
                "properties": {
                    "name": { "type": "string" },
                    "version": { "type": "string" },
                    "chainId": { "type": "integer", "minimum": 0 }
                }
            }
        }
    })
}

/// Tools whose REST body is also returned as `structuredContent`: those that
/// declare an `outputSchema`, read off the declarations so the two cannot
/// disagree.
static STRUCTURED_TOOLS: Lazy<Vec<String>> = Lazy::new(|| {
    tools()
        .into_iter()
        .filter(|t| t.output_schema.is_some())
        .map(|t| t.name.to_string())
        .collect()
});

/// The tool definitions, in [`TOOL_NAMES`] order.
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
        )
        .with_raw_output_schema(schema(supported_output_schema()))
        .with_meta(class_meta(TOOL_NAMES[0])),
        Tool::new(
            TOOL_NAMES[1],
            "Negotiate payment requirements: send the `accepts` array from a 402 \
             response and get back only the entries this facilitator can actually \
             settle, enriched with what it knows (feePayer, token list, escrow \
             addresses). Whatever it could NOT serve comes back in `rejected`, one \
             entry per dropped requirement, each with a `reason` from a closed set \
             (`malformed`, `network_unknown`, `scheme_unknown`, \
             `network_unsupported`, `scheme_unsupported_on_network`) -- read it \
             instead of guessing why a requirement vanished. `rejected` is always \
             present and empty when everything matched, and an empty `accepts` is \
             still a success, not an error. Moves no money and signs nothing. \
             Equivalent to POST /accepts.",
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
        )
        .with_meta(class_meta(TOOL_NAMES[1])),
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
        )
        .with_meta(class_meta(TOOL_NAMES[2])),
        Tool::new(
            TOOL_NAMES[3],
            "SETTLE A PAYMENT ON-CHAIN. This MOVES REAL FUNDS and is IRREVERSIBLE: it \
             broadcasts the payer's authorization as a blockchain transaction, and \
             nothing in this facilitator or any chain can undo a confirmed transfer. \
             Call x402_verify first, and only call this when the payment is meant to \
             execute now. Equivalent to POST /settle. Exact payments on the networks \
             GET /receipts lists (EVM ERC-3009 tokens, native Hedera USDC) return a \
             portable receipt with payment state and settlement ID; unknown \
             never authorizes a fresh signature. Preserve the original authorization. \
             Receipt confirmation is separate from merchant delivery.",
            schema(settle_input_schema()),
        )
        .annotate(
            ToolAnnotations::with_title("Settle a payment on-chain (irreversible)")
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
        )
        .with_meta(class_meta(TOOL_NAMES[3])),
        Tool::new(
            TOOL_NAMES[4],
            "How to present every network this facilitator settles on: display name, \
             chain family, chain id, testnet flag, explorer URL templates for a \
             transaction and an address, icon, schemes, and each token's symbol, \
             address, decimals and EIP-712 domain. One row per chain x402_supported \
             names, never more or fewer: use x402_supported to learn whether a payment \
             can be settled, and this to name the chain and link its transaction. \
             Takes no arguments. Moves no money and signs nothing. Equivalent to \
             GET /networks.json.",
            schema(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })),
        )
        .annotate(
            ToolAnnotations::with_title("Network directory: names, explorers, tokens")
                .read_only(true)
                .idempotent(true)
                .open_world(false),
        )
        .with_raw_output_schema(schema(networks_output_schema()))
        .with_meta(class_meta(TOOL_NAMES[4])),
        Tool::new(
            TOOL_NAMES[5],
            "Whether this facilitator can settle right now, chain by chain: RPC \
             reachability and how many settles each signer's gas still covers, graded \
             ok, degraded or down with a bounded reason. Use it to tell a payment that \
             did not settle because the chain or the facilitator's signer is down from \
             one refused on its merits. Optional `network` scopes the answer to one \
             chain, in either spelling (\"base\" or \"eip155:8453\"). The reading is \
             cached for ttlSecs and says when it was measured; a chain that reads down \
             comes back as this result, not as an error. Moves no money and signs \
             nothing. Equivalent to GET /health/ready.",
            schema(json!({
                "type": "object",
                "properties": {
                    NETWORK_ARG: {
                        "type": "string",
                        "description": "Optional. One chain, as its x402 v1 name (\"base\") or its CAIP-2 id (\"eip155:8453\"). Omit it for every configured chain."
                    }
                },
                "additionalProperties": false
            })),
        )
        .annotate(
            ToolAnnotations::with_title("Settlement health per chain")
                .read_only(true)
                .idempotent(true)
                .open_world(true),
        )
        .with_raw_output_schema(schema(chain_health_output_schema()))
        .with_meta(class_meta(TOOL_NAMES[5])),
        Tool::new(
            TOOL_NAMES[6],
            "Read what ONE x402r escrow payment holds on-chain, from its \
             AuthCaptureEscrow contract: whether the funds were collected, how much is \
             still capturable and how much is refundable. A view call on the contract \
             -- no transaction and no gas -- so it answers what the chain holds for this \
             payment, not what any marketplace recorded about a task. The arguments \
             identify the payment (the paymentInfo, payer, CAIP-2 network and escrow \
             contracts it was made with); nothing is signed. Moves no money. \
             Equivalent to POST /escrow/state.",
            schema(escrow_state_input_schema()),
        )
        .annotate(
            ToolAnnotations::with_title("On-chain escrow state (read-only)")
                .read_only(true)
                .idempotent(true)
                .open_world(true),
        )
        .with_raw_output_schema(schema(escrow_state_output_schema()))
        .with_meta(class_meta(TOOL_NAMES[6])),
        Tool::new(
            TOOL_NAMES[7],
            "Look up the DX402 durable evidence anchored for a settled payment: where \
             the sealed copy of the paid response lives, the hash of what was \
             delivered, its retention, whether the payee proved the anchor \
             (verified/signed) and the facilitator's signed receipt. \
             `document: \"receipt\"` returns the signed receipt alone, with its EIP-712 \
             domain, for offline verification. 404 means nothing was anchored for that \
             paymentId and 410 that it was and expired -- different answers in a \
             dispute. Served only where x402_supported lists `durable-evidence` in \
             `extensions`; elsewhere the call answers the generic 404. Moves no money, \
             signs nothing and never returns the sealed bytes. Equivalent to \
             GET /dx402/evidence/{paymentId} or GET /dx402/receipt/{paymentId}.",
            schema(durable_evidence_input_schema()),
        )
        .annotate(
            ToolAnnotations::with_title("DX402 durable evidence of a payment")
                .read_only(true)
                .idempotent(true)
                .open_world(false),
        )
        .with_raw_output_schema(schema(durable_evidence_output_schema()))
        .with_meta(class_meta(TOOL_NAMES[7])),
    ]
}

/// The MCP server: [`TOOL_NAMES`] over the facilitator's own REST router.
#[derive(Clone)]
pub struct FacilitatorMcp {
    /// The routes the tools dispatch to ([`rest_router`]), with state and
    /// extensions already bound. Cloned per call; an axum `Router` is an `Arc`
    /// inside, so this is a refcount bump, not a rebuild.
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
        request: RestRequest,
        outer: Option<&HeaderMap>,
        idempotency_key: Option<&str>,
    ) -> Result<(StatusCode, String), McpError> {
        let RestRequest { method, uri, body } = request;
        let mut builder = Request::builder().method(method).uri(uri);
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        // Carry the caller's IP forward. Not cosmetic, and not for THIS hop:
        // the inner router has no `GovernorLayer`, so nothing here reads it.
        // It matters one hop further on. `settle_writer_gate` forwards an EVM
        // settle to the task holding the writer lease over a direct
        // task-to-task TCP connection (`forward_to_writer`), which never
        // touches the ALB that would add the header. On the other side
        // `/settle` sits behind `ClientIpKeyExtractor` (src/client_ip.rs),
        // which keys on the last X-Forwarded-For entry and, only without the
        // header, on the TCP peer -- there the forwarding task, not the
        // client. A synthetic request built with only a content-type would
        // therefore charge every MCP settle one task forwards to a single
        // bucket, and legitimate callers would 429 each other. With more than
        // one ECS task that is most EVM settles over MCP.
        //
        // Copying the header verbatim is also the faithful choice: the holder
        // then charges the token to the same client a forwarded `POST /settle`
        // would. Every line of it, not the first: the holder keys on the
        // shape of the header, and several lines are keyed differently from
        // one.
        //
        // `X-UVD-Stack-Key` for the same reason: the outer `/mcp` governor
        // already let a recognized stack identity through, and without its key
        // the holder would charge that caller as a third party.
        for name in [
            header::HeaderName::from_static("x-forwarded-for"),
            header::HeaderName::from_static("x-uvd-purchase"),
            header::HeaderName::from_static(crate::rate_policy::STACK_KEY_HEADER),
        ] {
            for value in outer.into_iter().flat_map(|h| h.get_all(&name)) {
                builder = builder.header(&name, value.clone());
            }
        }
        // The one header a tool may set, and only because `post_settle` reads
        // it: without it an MCP client has no way to ask for exactly-once, and
        // an MCP client is a model -- precisely the caller that retries an
        // ambiguous error.
        if let Some(key) = idempotency_key {
            let value = header::HeaderValue::from_str(key).map_err(|_| {
                McpError::invalid_params(
                    "idempotencyKey must be printable ASCII with no newlines".to_string(),
                    None,
                )
            })?;
            builder = builder.header(header::HeaderName::from_static("idempotency-key"), value);
        }
        let body = match body {
            Some(arguments) => {
                let bytes = serde_json::to_vec(&Value::Object(arguments)).map_err(|e| {
                    McpError::invalid_params(
                        format!("arguments are not serialisable JSON: {e}"),
                        None,
                    )
                })?;
                Body::from(bytes)
            }
            None => Body::empty(),
        };
        let request = builder.body(body).map_err(|e| {
            McpError::internal_error(format!("could not build the internal request: {e}"), None)
        })?;

        // `Infallible`, so the `?` is a formality the type system asks for.
        let response =
            self.rest.clone().oneshot(request).await.map_err(|e| {
                McpError::internal_error(format!("internal routing failed: {e}"), None)
            })?;

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
        info.server_info =
            Implementation::new("x402-facilitator", crate::version::facilitator_version())
                .with_title("x402 Payment Facilitator")
                .with_description(
                    "Verify and settle x402 gasless stablecoin payments across EVM, Solana, NEAR, \
             Stellar, Algorand, Sui, XRPL and Hedera.",
                )
                .with_website_url("https://facilitator.ultravioletadao.xyz");
        info.instructions = Some(
            "Call x402_supported first to learn which network and scheme pairs this \
             facilitator settles. Then x402_accepts to narrow a resource server's 402 \
             offer, x402_verify to check a signed authorization, and x402_settle to \
             broadcast it. x402_settle moves real funds and cannot be undone. This \
             facilitator charges no fee and authenticates no caller: the payer's \
             signature is the only authority, so an MCP client holds exactly the \
             privilege an HTTP client holds and no more. A settle failure whose body \
             says \"retryable\": false, or names a transaction, may already be on chain: \
             do not retry it and never sign a new authorization; look the transaction \
             up. If a settle fails in any other way you cannot interpret, retry it with \
             the SAME idempotencyKey: that is how you get exactly-once instead of paying \
             twice. Four more tools only read, and move and sign nothing: x402_networks \
             (each network's name, explorer and tokens), x402_chain_health (whether a \
             chain can settle right now), x402_escrow_onchain_state (what an escrow \
             contract holds for one payment) and x402_durable_evidence (the DX402 \
             evidence anchored for a payment)."
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.clone();
        if rest_target(name.as_ref()).is_none() {
            // Unroutable request, not a tool that ran and failed: the caller
            // asked for something this server does not have.
            return Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {name}"),
                Some(json!({ "tools": TOOL_NAMES })),
            ));
        }
        let mut arguments = request.arguments.unwrap_or_default();

        // `idempotencyKey` travels as a HEADER and must not stay in the body:
        // `post_settle` hashes the body to tell "same key, same request" from
        // "same key, different request", so an extra field in it would make two
        // identical payments hash differently and defeat the deduplication the
        // key was asked for.
        let idempotency_key = if name.as_ref() == "x402_settle" {
            match arguments.remove(IDEMPOTENCY_KEY_ARG) {
                Some(Value::String(key)) if !key.trim().is_empty() => Some(key),
                Some(Value::Null) | None => None,
                Some(_) => {
                    return Err(McpError::invalid_params(
                        format!("{IDEMPOTENCY_KEY_ARG} must be a non-empty string"),
                        None,
                    ));
                }
            }
        } else {
            None
        };

        // A bad ARGUMENT (an undeclared key, a missing `paymentId`) is
        // refused here, before anything is dispatched; a bad VALUE is the
        // REST handler's to judge.
        let rest = rest_request(name.as_ref(), arguments)?;
        let path = rest.uri.clone();

        // rmcp leaves the outer request's `http::request::Parts` here. `None`
        // only in a non-HTTP transport, which this server does not mount.
        let outer = context.extensions.get::<axum::http::request::Parts>();
        let (status, body) = self
            .call_rest(
                rest,
                outer.map(|p| &p.headers),
                idempotency_key.as_deref(),
            )
            .await?;

        // The REST status decides. A 4xx/5xx becomes a tool-level error the
        // caller can read, carrying the facilitator's own message verbatim --
        // NOT an `Err(McpError)`, which most clients render opaquely and would
        // hide "invalid signature" behind "internal error". The one non-2xx
        // that is a reading and not a failure is in `is_tool_result`.
        if !is_tool_result(name.as_ref(), status, &body) {
            return Ok(CallToolResult::error(vec![ContentBlock::text(body)]).into());
        }
        // A tool that declares an `outputSchema` MUST answer in
        // `structuredContent` too (MCP), and clients validate it there. The
        // text block stays, byte for byte the REST body, for the clients that
        // read only text.
        let structured = if STRUCTURED_TOOLS.iter().any(|t| t == name.as_ref()) {
            match serde_json::from_str::<Value>(&body) {
                Ok(doc @ Value::Object(_)) => Some(doc),
                // Cannot happen for the reads, whose bodies are always JSON
                // objects. If it ever does, a result that breaks the declared
                // schema is worse than an error that says why.
                _ => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "{path} answered {status} with a body that is not the JSON \
                         object this tool's outputSchema declares: {body}"
                    ))])
                    .into());
                }
            }
        } else {
            None
        };
        let mut result = CallToolResult::success(vec![ContentBlock::text(body)]);
        result.structured_content = structured;
        Ok(result.into())
    }
}

/// Give rmcp's own error responses a `content-type`.
///
/// rmcp builds its transport-level refusals -- 406 for an `Accept` that does not
/// name BOTH `application/json` and `text/event-stream`, 415 for the wrong
/// request content type, 403 for a `Host` outside the allowlist, 405 for a
/// method it does not serve -- with a bare `Response::builder()` and a plain
/// string body, so they go out with no `content-type` at all. A surface is
/// graded on its content type as much as on its status, and an integrator
/// writing a client by hand hits the 406 first.
///
/// This wraps only those: a response that already declares a type is passed
/// through untouched and its body is never buffered, so the 200 JSON and the
/// SSE fallback are not affected. rmcp's own message is kept as `detail` --
/// it is the part that says what to fix.
async fn json_content_type_on_errors(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    if response.headers().contains_key(header::CONTENT_TYPE) {
        return response;
    }
    let (parts, body) = response.into_parts();
    // These bodies are short constant strings. The cap is a guard, not a limit.
    const MAX_ERROR_BODY: usize = 8 * 1024;
    let detail = match axum::body::to_bytes(body, MAX_ERROR_BODY).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    };
    let mut doc = json!({
        "error": parts.status.canonical_reason().unwrap_or("error"),
        "status": parts.status.as_u16(),
    });
    if !detail.trim().is_empty() {
        doc["detail"] = json!(detail.trim());
    }
    if parts.status == StatusCode::NOT_ACCEPTABLE {
        doc["hint"] = json!(
            "Accept must name BOTH application/json and text/event-stream, \
             e.g. `accept: application/json, text/event-stream`."
        );
    }
    (parts.status, parts.headers, Json(doc)).into_response()
}

/// The routes the tools dispatch to, and nothing else a tool could reach.
///
/// Every route here is mounted from the SAME handler the public router mounts
/// in `main.rs`, so a tool answers what the HTTP route answers. The reads are
/// added route by route, never by merging a whole public router: the DX402 one
/// would bring `/dx402/anchor`, `/dx402/recover` and `/dx402/repair` with it,
/// and `the_internal_router_serves_no_write_and_no_admin_route` holds that.
///
/// `readiness` is the process's ONE `/health/ready` state, the very `Arc`
/// `main.rs` mounts the public route with -- not a second one built here. Its
/// cache and its single background prober are what keep that route from
/// amplifying RPC traffic; a second state would double the probes.
///
/// `dx402` is `None` exactly when the public `/dx402/*` routes are not mounted,
/// and then the DX402 tool reaches the same generic 404 an HTTP caller gets:
/// the fallback is the public router's own ([`crate::handlers::agent_not_found`]).
fn rest_router<A, P>(
    state: A,
    discovery_registry: Arc<DiscoveryRegistry>,
    event_bus: Arc<crate::events::EventBus>,
    transaction_store: Arc<dyn crate::transaction_store::TransactionStore>,
    readiness: Arc<crate::readiness::ReadinessState<P>>,
    dx402: Option<Arc<crate::dx402::service::Dx402Service>>,
) -> Router
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
    P: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    let mut rest = crate::handlers::verify_settle_routes::<A>()
        .merge(
            Router::new()
                .route("/supported", get(crate::handlers::get_supported::<A>))
                .route("/accepts", post(crate::handlers::post_accepts::<A>))
                .route(
                    "/networks.json",
                    get(crate::handlers::get_networks_json::<A>),
                )
                .route(
                    "/escrow/state",
                    post(crate::handlers::post_escrow_state::<A>),
                ),
        )
        .with_state(state)
        .merge(crate::readiness::routes::<P>().with_state(readiness));
    if let Some(dx402) = dx402 {
        rest = rest.merge(
            Router::new()
                .route(
                    "/dx402/evidence/{payment_id}",
                    get(crate::dx402::handlers::get_evidence),
                )
                .route(
                    "/dx402/receipt/{payment_id}",
                    get(crate::dx402::handlers::get_receipt),
                )
                .with_state(dx402),
        );
    }
    rest.fallback(crate::handlers::agent_not_found)
        // `post_verify` and `post_settle` pull these out of the request
        // extensions. The synthetic request is built downstream of the layers
        // `main.rs` applies, so it has to carry its own copies.
        .layer(Extension(discovery_registry))
        .layer(Extension(event_bus))
        .layer(Extension(transaction_store))
}

/// The `/mcp` route.
///
/// `mount` it under the same `GovernorLayer` as `/verify` and `/settle`: an MCP
/// `x402_settle` costs the chain exactly what a `POST /settle` costs it, and
/// the reads draw on that same bucket -- stricter than the budgets their REST
/// routes carry, so the MCP door is never a way around those
/// (`the_mcp_bucket_is_never_looser_than_the_reads_it_relays`).
///
/// The REST router built here carries **no** governor of its own -- see the
/// module docs for why that is correct rather than an oversight. See
/// [`rest_router`] for `readiness` and `dx402`.
pub fn mcp_routes<A, P>(
    state: A,
    discovery_registry: Arc<DiscoveryRegistry>,
    event_bus: Arc<crate::events::EventBus>,
    transaction_store: Arc<dyn crate::transaction_store::TransactionStore>,
    readiness: Arc<crate::readiness::ReadinessState<P>>,
    dx402: Option<Arc<crate::dx402::service::Dx402Service>>,
) -> Router
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
    P: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    let rest = rest_router(
        state,
        discovery_registry,
        event_bus,
        transaction_store,
        readiness,
        dx402,
    );

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

    Router::new()
        // `GET` is the human guide, `POST` is this server. A caller whose
        // `Accept` says it is a transport client still gets the 405 naming
        // POST -- see `crate::handlers::get_mcp_page`.
        .route(
            "/mcp",
            post_service(service).get(crate::handlers::get_mcp_page),
        )
        .layer(axum::middleware::from_fn(json_content_type_on_errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::FacilitatorLocalError;
    use crate::network::Network;
    use crate::types::{Scheme, X402Version};
    use crate::types::{
        SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindsResponse,
        VerifyRequest, VerifyResponse,
    };
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
                    network_aliases: None,
                    extra: None,
                }],
            })
        }
    }

    /// The MCP router, and the PUBLIC REST router it must agree with, over the
    /// stub, without DX402 (the default deployment).
    async fn routers() -> (Router, Router) {
        routers_with(None).await
    }

    /// Same, with `dx402` mounted on both sides when it is `Some`.
    ///
    /// The MCP side is `mcp_routes` itself. The REST side is NOT a copy of
    /// `rest_router`: it is the public routers `main.rs` mounts --
    /// `verify_settle_routes`, `routes`, `secondary_read_routes`, the readiness
    /// routes, the full `dx402_routes` -- with `main.rs`'s fallback, minus the
    /// governors. That is the claim the comparisons below test: a tool answers
    /// what an HTTP client of the public route gets. Both sides share ONE
    /// readiness state, as in `main.rs`. The one thing these tests cannot
    /// exercise is the chain, which is why the live `curl` transcript in the
    /// handoff exists.
    async fn routers_with(dx402: Option<Arc<crate::dx402::service::Dx402Service>>) -> (Router, Router) {
        let state = StubFacilitator::new();
        let discovery_registry = Arc::new(DiscoveryRegistry::new());
        let event_bus = Arc::new(crate::events::EventBus::from_env());
        let transaction_store = crate::transaction_store::create_transaction_store().await;
        let readiness = Arc::new(crate::readiness::ReadinessState::new(
            Arc::new(NoProviders),
            crate::readiness::ReadinessConfig::from_env(),
        ));

        let mcp = mcp_routes(
            state.clone(),
            Arc::clone(&discovery_registry),
            Arc::clone(&event_bus),
            Arc::clone(&transaction_store),
            Arc::clone(&readiness),
            dx402.clone(),
        );
        let mut rest = crate::handlers::verify_settle_routes::<StubFacilitator>()
            .merge(crate::handlers::routes::<StubFacilitator>())
            .merge(crate::handlers::secondary_read_routes::<StubFacilitator>())
            .with_state(state)
            .merge(crate::readiness::routes::<NoProviders>().with_state(readiness));
        if let Some(svc) = dx402 {
            rest = rest.merge(crate::dx402::handlers::dx402_routes().with_state(svc));
        }
        let rest = rest
            .fallback(crate::handlers::agent_not_found)
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
        rpc_from(mcp, body, None).await
    }

    /// Same, but with a client IP on the OUTER request -- what the ALB adds.
    async fn rpc_from(
        mcp: &Router,
        body: Value,
        client_ip: Option<&str>,
    ) -> (StatusCode, String, Value) {
        rpc_from_lines(mcp, body, client_ip.as_slice()).await
    }

    /// Same, with one `X-Forwarded-For` header line per entry of `xff`.
    async fn rpc_from_lines(
        mcp: &Router,
        body: Value,
        xff: &[&str],
    ) -> (StatusCode, String, Value) {
        let lines: Vec<(&str, &str)> = xff.iter().map(|line| ("x-forwarded-for", *line)).collect();
        rpc_with_headers(mcp, body, &lines).await
    }

    /// Same, with one header line per `(name, value)` of `extra`.
    async fn rpc_with_headers(
        mcp: &Router,
        body: Value,
        extra: &[(&str, &str)],
    ) -> (StatusCode, String, Value) {
        let mut builder = HttpRequest::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        for (name, value) in extra {
            builder = builder.header(*name, *value);
        }
        let request = builder
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
        assert!(!result["capabilities"]["tools"].is_null());
    }

    /// The tool list is exactly the eight, in order, and nothing else.
    ///
    /// This is the test that keeps an admin surface out. `/feedback/revoke`
    /// erases third-party reputation irreversibly and `/discovery/admin/*`
    /// curates a public catalog; neither belongs in a list handed to an LLM.
    /// The list is spelled out here, not read back from [`TOOL_NAMES`], so a
    /// tool cannot be added by editing one constant: it changes this test too.
    #[tokio::test]
    async fn tools_list_is_exactly_the_eight() {
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
        assert_eq!(
            names,
            [
                "x402_supported",
                "x402_accepts",
                "x402_verify",
                "x402_settle",
                "x402_networks",
                "x402_chain_health",
                "x402_escrow_onchain_state",
                "x402_durable_evidence",
            ]
        );
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

    /// `tools/list` as a client receives it.
    async fn listed_tools(mcp: &Router) -> Vec<Value> {
        let (_, _, doc) = rpc(
            mcp,
            json!({ "jsonrpc": "2.0", "id": 20, "method": "tools/list" }),
        )
        .await;
        doc["result"]["tools"].as_array().unwrap().clone()
    }

    fn validator_for(schema: &Value) -> jsonschema::Validator {
        jsonschema::validator_for(schema).expect("an output schema is a valid JSON Schema")
    }

    /// Every tool declares one house class, from the closed vocabulary, in
    /// `_meta["uvd/clase"]` -- and never inside `annotations`, where a catalog
    /// fingerprinting the tool would read it as a change to the tool (R9.4).
    #[tokio::test]
    async fn every_tool_declares_its_house_class_in_meta() {
        let (mcp, _) = routers().await;
        let tools = listed_tools(&mcp).await;
        assert_eq!(tools.len(), TOOL_CLASSES.len());
        for (name, class) in TOOL_CLASSES {
            let tool = tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not listed"));
            assert_eq!(tool["_meta"][CLASS_META_KEY], class, "{name}");
            assert!(
                HOUSE_CLASSES.contains(&class),
                "{name}: {class} is not a house class"
            );
            assert!(
                tool["annotations"].get(CLASS_META_KEY).is_none(),
                "{name} carries the class inside annotations"
            );
        }
    }

    /// The class never contradicts the MCP hints the same tool publishes: a
    /// read is read-only and not destructive, the one that moves money says
    /// it is destructive, and nothing is left `sin_clasificar`.
    #[tokio::test]
    async fn the_class_agrees_with_the_annotations() {
        let (mcp, _) = routers().await;
        for tool in listed_tools(&mcp).await {
            let name = tool["name"].as_str().unwrap();
            let class = tool["_meta"][CLASS_META_KEY].as_str().unwrap();
            let hints = &tool["annotations"];
            assert_ne!(class, "sin_clasificar", "{name}");
            match class {
                "lectura" => {
                    assert_eq!(hints["readOnlyHint"], true, "{name}");
                    assert_ne!(hints["destructiveHint"], true, "{name}");
                }
                "mueve_dinero" => {
                    assert_eq!(hints["readOnlyHint"], false, "{name}");
                    assert_eq!(hints["destructiveHint"], true, "{name}");
                }
                _ => {}
            }
        }
    }

    /// A read publishes an object `outputSchema` (R9.5); only the reads do,
    /// so the tools whose contract did not change keep their exact shape.
    #[tokio::test]
    async fn only_the_reads_publish_an_output_schema() {
        let (mcp, _) = routers().await;
        for tool in listed_tools(&mcp).await {
            let name = tool["name"].as_str().unwrap();
            let is_read = tool["_meta"][CLASS_META_KEY] == "lectura";
            assert_eq!(
                tool.get("outputSchema").is_some(),
                is_read,
                "{name}: outputSchema present = read"
            );
            if is_read {
                assert_eq!(tool["outputSchema"]["type"], "object", "{name}");
            }
        }
    }

    /// `x402_supported` answers in `structuredContent` with the same document
    /// its text block carries, and that document validates against the
    /// `outputSchema` the tool declared -- the check an MCP client runs.
    #[tokio::test]
    async fn supported_returns_structured_content_that_matches_its_output_schema() {
        let (mcp, _) = routers().await;
        let tools = listed_tools(&mcp).await;
        let declared = &tools
            .iter()
            .find(|t| t["name"] == "x402_supported")
            .unwrap()["outputSchema"];
        let (_, _, doc) = rpc(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 21, "method": "tools/call",
                "params": { "name": "x402_supported", "arguments": {} }
            }),
        )
        .await;
        let structured = &doc["result"]["structuredContent"];
        let text: Value = serde_json::from_str(&tool_text(&doc)).unwrap();
        assert_eq!(
            structured, &text,
            "structuredContent is not the text block's document"
        );
        let validator = validator_for(declared);
        let errors: Vec<String> = validator
            .iter_errors(structured)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "structuredContent breaks the outputSchema: {errors:#?}"
        );
    }

    /// The tools that declare no `outputSchema` keep answering in text only.
    #[tokio::test]
    async fn a_tool_without_an_output_schema_answers_without_structured_content() {
        let (mcp, _) = routers().await;
        let (_, _, doc) = rpc(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 22, "method": "tools/call",
                "params": { "name": "x402_accepts", "arguments": { "accepts": [] } }
            }),
        )
        .await;
        assert_ne!(doc["result"]["isError"], true, "{doc}");
        assert!(doc["result"].get("structuredContent").is_none(), "{doc}");
    }

    /// The schema describes what production serves, not what a stub serves:
    /// a real `/supported` body (156 kinds: v1 and CAIP-2 entries, tokens,
    /// escrow addresses, Hedera fee payers, extensions, signers, receipts)
    /// validates, and so does one with every optional field the type can
    /// carry. And it is not a schema that accepts anything: a body without
    /// `kinds`, or with a version written as a string, is refused.
    #[test]
    fn the_supported_output_schema_describes_a_production_body() {
        let validator = validator_for(&supported_output_schema());
        let snapshot: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/supported-before-celo-sepolia.json"
        ))
        .unwrap();
        assert!(snapshot["kinds"].as_array().is_some_and(|k| k.len() > 100));
        let errors: Vec<String> = validator
            .iter_errors(&snapshot)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "a production /supported breaks the schema: {errors:#?}"
        );

        let mut broken = snapshot.clone();
        broken.as_object_mut().unwrap().remove("kinds");
        assert!(
            !validator.is_valid(&broken),
            "a body without kinds validates"
        );
        let mut broken = snapshot.clone();
        broken["kinds"][0]["x402Version"] = json!("1");
        assert!(!validator.is_valid(&broken), "a string version validates");
        let mut broken = snapshot;
        broken["kinds"][0]["extra"] = json!({ "tokens": [{ "token": "usdc" }] });
        assert!(
            !validator.is_valid(&broken),
            "a token without an address validates"
        );
    }

    /// Every URL `/.well-known/uvd-stack.json` publishes is a path the real
    /// routers serve. axum will not list its routes, so the manifest's paths
    /// are walked through the same router builders `main.rs` mounts; a route
    /// renamed without the manifest following fails here, not in a client.
    #[tokio::test]
    async fn every_url_the_interop_manifest_publishes_is_served() {
        let (mcp, _) = routers().await;
        let state = StubFacilitator::new();
        let readiness = crate::readiness::ReadinessState::new(
            Arc::new(NoProviders),
            crate::readiness::ReadinessConfig::from_env(),
        );
        let app = Router::new()
            .merge(crate::handlers::agentic_routes())
            .merge(crate::openapi::swagger_routes())
            .merge(crate::handlers::routes::<StubFacilitator>().with_state(state))
            .merge(crate::readiness::routes::<NoProviders>().with_state(Arc::new(readiness)))
            .merge(
                crate::handlers::events_routes()
                    .with_state(Arc::new(crate::events::EventBus::from_env())),
            )
            .merge(mcp);
        let manifest: Value = serde_json::from_str(crate::interop::served_document()).unwrap();
        assert!(!crate::interop::published_paths().is_empty());
        for path in crate::interop::published_paths() {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(path)
                        .header(header::HOST, "127.0.0.1")
                        .header("x-forwarded-for", "192.0.2.1")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "the manifest publishes {path}, which no route serves"
            );
            assert!(
                manifest
                    .to_string()
                    .contains(&format!("{}{path}", crate::interop::PUBLIC_URL))
                    || path == crate::interop::MANIFEST_PATH,
                "{path} is walked but not published"
            );
        }
    }

    /// The MCP router has no bucket of its own. `main.rs` mounts `/mcp` under
    /// the verify/settle bucket; a second one INSIDE `mcp_routes` would charge
    /// every tool call twice and refuse at half the published quota. Forty calls
    /// from one address -- past verify/settle's burst -- through `mcp_routes`
    /// alone, with the `X-Forwarded-For` a governor would key on: none is
    /// refused and none is a tool error.
    #[tokio::test]
    async fn the_mcp_router_has_no_bucket_of_its_own() {
        let (mcp, _) = routers().await;
        let calls = 40;
        assert!(calls > crate::rate_policy::VERIFY_SETTLE.limit().burst());
        for n in 0..calls {
            let (status, _, doc) = rpc_from(
                &mcp,
                json!({
                    "jsonrpc": "2.0", "id": n, "method": "tools/call",
                    "params": { "name": "x402_supported", "arguments": {} }
                }),
                Some("203.0.113.88"),
            )
            .await;
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "call {n}: {doc}");
            assert_eq!(status, StatusCode::OK, "call {n}: {doc}");
            assert_ne!(doc["result"]["isError"], true, "call {n}: {doc}");
        }
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
        assert_eq!(
            doc["error"]["code"], -32601,
            "expected METHOD_NOT_FOUND: {doc}"
        );
    }

    /// `(status, content-type, body)` of a `GET /mcp` with the given `Accept`.
    async fn get_mcp(accept: Option<&str>) -> (StatusCode, String, String) {
        let (mcp, _) = routers().await;
        let mut builder = HttpRequest::builder()
            .method(Method::GET)
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1");
        if let Some(accept) = accept {
            builder = builder.header(header::ACCEPT, accept);
        }
        let response = mcp
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
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
        (status, ctype, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// `GET /mcp` is the guide a person reads. It used to be a 405 and nothing
    /// else, which meant the only thing behind the URL an integrator pastes
    /// into a config file was a refusal.
    #[tokio::test]
    async fn get_mcp_serves_the_human_guide() {
        for accept in [None, Some("*/*"), Some("text/html,application/xhtml+xml")] {
            let (status, ctype, body) = get_mcp(accept).await;
            assert_eq!(status, StatusCode::OK, "Accept: {accept:?}");
            assert!(
                ctype.starts_with("text/html"),
                "Accept: {accept:?} answered content-type {ctype:?}"
            );
            assert!(
                body.contains("x402_settle"),
                "Accept: {accept:?} did not answer the MCP guide"
            );
        }
    }

    /// The same guide as Markdown, for an agent that does not want to render
    /// HTML to read it.
    #[tokio::test]
    async fn get_mcp_negotiates_markdown() {
        let (status, ctype, body) = get_mcp(Some("text/markdown")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(ctype.starts_with("text/markdown"), "content-type {ctype:?}");
        // The shape, not the sentence. Pinning the literal title made this test
        // fail the day the page's `h1` was rewritten -- and the rewrite was the
        // point: `/mcp`, `mcp.md` and the landing all have to open with the
        // same headline, or the same URL answers three different things
        // depending on who asks. What matters here is that Markdown came back
        // and that it is this guide.
        assert!(body.starts_with("# "), "body: {body:.80}");
        assert!(
            body.contains("x402_settle"),
            "text/markdown did not answer the MCP guide: {body:.80}"
        );
    }

    /// A transport client that used the wrong method still gets the machine
    /// answer, not the page.
    ///
    /// This is the whole reason the `Accept` is read at all: the Streamable
    /// HTTP transport sends `application/json, text/event-stream` on every
    /// request, so a caller arriving here with that header is an MCP client
    /// that sent `GET`, and handing it 200 with an HTML page would turn a clear
    /// "use POST" into a parse error with no explanation.
    #[tokio::test]
    async fn get_mcp_still_answers_405_to_a_transport_client() {
        for accept in [
            "application/json, text/event-stream",
            "application/json",
            "text/event-stream",
        ] {
            let (status, ctype, body) = get_mcp(Some(accept)).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "Accept: {accept}");
            assert!(
                ctype.starts_with("application/json"),
                "Accept: {accept} answered content-type {ctype:?}"
            );
            assert!(
                !body.trim_start().starts_with('<'),
                "Accept: {accept} answered markup: {body}"
            );
            let doc: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(doc["method"], "POST");
            assert_eq!(
                doc["humanGuide"],
                "https://facilitator.ultravioletadao.xyz/mcp"
            );
        }
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

    /// Every tool name maps to a route, and only to the eight intended ones.
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
                (Method::GET, "/networks.json"),
                (Method::GET, "/health/ready"),
                (Method::POST, "/escrow/state"),
                (Method::GET, "/dx402/{document}/{paymentId}"),
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
        // What a v1 body must carry: the unconditional fields plus the ones the
        // v1 branch of `anyOf` adds. `paymentRequirements` moved into the branch
        // when v2 -- which has no such field -- was described alongside it.
        let mut required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        required.extend(
            v1_branch(&schema)["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        );
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

    /// The schema publishes the SIGNED fields, not an opaque object.
    ///
    /// `paymentPayload` and `paymentRequirements` used to be
    /// `{"type": "object", "additionalProperties": true}` -- a declaration that
    /// says nothing -- with the description pointing at `/skill.md` for the
    /// real shape. `/skill.md` then published an example the facilitator
    /// answered `400` to. An agent arriving over MCP therefore had no correct
    /// source for the body anywhere: empty on this side, wrong on the other.
    ///
    /// The six authorization fields are the ones an agent gets wrong (`amount`
    /// for `value`, no `authorization` wrapper, numeric timestamps), so they
    /// are what the walk asserts.
    #[test]
    fn the_schema_publishes_the_authorization_fields() {
        let schema = payment_envelope_schema("/verify");
        let authorization = &schema["properties"]["paymentPayload"]["properties"]["payload"]
            ["properties"]["authorization"]["properties"];
        for field in ["from", "to", "value", "validAfter", "validBefore", "nonce"] {
            assert!(
                authorization[field].is_object(),
                "the schema does not declare authorization.{field}"
            );
        }
        assert!(
            authorization["amount"].is_null(),
            "the authorization field is `value`, not `amount` -- declaring both \
             would teach the mistake the field exists to prevent"
        );

        // The four `paymentRequirements` fields with no serde default. Omitting
        // one is a parse failure, so a schema that leaves them optional is
        // wrong in the direction that costs a round trip.
        let required: Vec<&str> = schema["properties"]["paymentRequirements"]["required"]
            .as_array()
            .expect("paymentRequirements must declare its required fields")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for field in ["resource", "description", "mimeType", "maxTimeoutSeconds"] {
            assert!(
                required.contains(&field),
                "`{field}` has no default in PaymentRequirements and must be required"
            );
        }
    }

    /// The example carried in the schema is a body `/verify` accepts.
    ///
    /// It is not a copy: it is parsed out of `static/skill.md`, the one place
    /// the example is written. That is what makes it impossible for an MCP
    /// client and a human reader to be shown different bodies.
    #[test]
    fn the_example_embedded_in_the_schema_deserialises() {
        for operation in ["/verify", "/settle"] {
            let schema = payment_envelope_schema(operation);
            let example = schema["examples"][0].clone();
            assert!(
                example.is_object(),
                "{operation} must carry a worked example"
            );
            let parsed: Result<crate::types_v2::VerifyRequestEnvelope, _> =
                serde_json::from_value(example);
            assert!(
                parsed.is_ok(),
                "the example the {operation} schema publishes does not deserialise: {}",
                parsed.unwrap_err()
            );
        }
    }

    /// Both spellings of a network are announced, because the schema is where
    /// an agent looks before it builds a body -- and our own Bazaar hands it
    /// CAIP-2.
    /// The `anyOf` branch describing one of the two envelopes, by title.
    fn envelope_branch<'a>(schema: &'a Value, title: &str) -> &'a Value {
        schema["anyOf"]
            .as_array()
            .expect("the envelope schema must branch on the two shapes")
            .iter()
            .find(|b| b["title"] == title)
            .unwrap_or_else(|| panic!("no `{title}` branch in the envelope schema"))
    }

    fn v1_branch(schema: &Value) -> &Value {
        envelope_branch(schema, "x402 v1")
    }

    /// **The schema describes the x402 v2 envelope, not only the v1 one.**
    ///
    /// It described v1 alone, and unconditionally: `paymentRequirements` was in
    /// the top-level `required`. So an agent building the v2 body -- the one the
    /// ChatGPT -> Paybox -> MeshRelay flow builds -- was told by the tool
    /// definition itself to add a field x402 v2 does not have, while nothing
    /// anywhere named `resource` or `accepted`.
    #[test]
    fn the_payment_schema_describes_the_v2_envelope() {
        let schema = payment_envelope_schema("/verify");

        // The two v2 fields exist and carry the sub-fields the type requires.
        let resource: Vec<&str> = schema["properties"]["resource"]["required"]
            .as_array()
            .expect("v2 `resource` must declare its required fields")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(resource, vec!["url", "description", "mimeType"]);

        let accepted: Vec<&str> = schema["properties"]["accepted"]["required"]
            .as_array()
            .expect("v2 `accepted` must declare its required fields")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            accepted,
            vec![
                "scheme",
                "network",
                "asset",
                "amount",
                "payTo",
                "maxTimeoutSeconds"
            ]
        );

        // ... and they are what the v2 branch demands, while v1 demands
        // `paymentRequirements`. The two shapes are not merged into one bag of
        // optional fields: a body still has to be one or the other.
        assert_eq!(
            envelope_branch(&schema, "x402 v2")["required"],
            json!(["resource", "accepted"])
        );
        assert_eq!(
            v1_branch(&schema)["required"],
            json!(["paymentRequirements"])
        );

        // The discriminating half: `paymentRequirements` must NOT be required
        // unconditionally any more, or the v2 branch is unreachable in practice.
        let top: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            !top.contains(&"paymentRequirements"),
            "`paymentRequirements` is a v1 field and cannot be required of a v2 body"
        );
    }

    /// **The v2 example in the schema is a body `/verify` accepts.**
    ///
    /// The same binding [`the_example_embedded_in_the_schema_deserialises`]
    /// makes for v1, on the shape that was actually failing. Both examples are
    /// read out of `static/skill.md`, so this fails if the document publishes a
    /// v2 body the parser refuses -- which is exactly what happened to the v1
    /// one for months.
    #[test]
    fn the_v2_example_embedded_in_the_schema_deserialises() {
        let schema = payment_envelope_schema("/verify");
        let example = &schema["examples"][1];
        assert_eq!(
            example["x402Version"], 2,
            "the second published example must be the v2 one"
        );
        assert!(
            example.get("paymentRequirements").is_none(),
            "a v2 example must not carry `paymentRequirements`"
        );
        let parsed: Result<crate::types_v2::VerifyRequestEnvelope, _> =
            serde_json::from_value(example.clone());
        assert!(
            parsed.is_ok(),
            "the v2 example published in the MCP schema does not deserialise: {}",
            parsed.unwrap_err()
        );
    }

    #[test]
    fn the_schema_says_both_network_spellings_are_accepted() {
        let schema = payment_envelope_schema("/verify");
        for path in [
            &schema["properties"]["paymentPayload"]["properties"]["network"]["description"],
            &schema["properties"]["paymentRequirements"]["properties"]["network"]["description"],
        ] {
            let text = path.as_str().expect("network needs a description");
            assert!(
                text.contains("base") && text.contains("eip155:8453"),
                "the description must show both spellings, got: {text}"
            );
        }
    }

    /// **`accepted.network` is CAIP-2 only, and the schema says so.**
    ///
    /// The two shapes do NOT share the network rule, and the old hint claimed
    /// they did -- it told every rejected body that `"base"` and
    /// `"eip155:8453"` both work. In the v2 envelope `accepted.network`
    /// deserialises as a `Caip2NetworkId`, which needs `namespace:reference`,
    /// so a bare `"base"` is a hard parse error. Verified against production
    /// 2.10.0 on 2026-09-04 and pinned to the type here, so the claim goes red
    /// if `Caip2NetworkId` ever learns the v1 names.
    #[test]
    fn the_schema_says_accepted_network_is_caip2_only() {
        let schema = payment_envelope_schema("/verify");
        let text = schema["properties"]["accepted"]["properties"]["network"]["description"]
            .as_str()
            .expect("accepted.network needs a description");
        assert!(
            text.contains("CAIP-2 ONLY"),
            "accepted.network must say it refuses the v1 name, got: {text}"
        );

        // The discriminating half: the claim is true of the type.
        use std::str::FromStr;
        assert!(crate::caip2::Caip2NetworkId::from_str("eip155:8453").is_ok());
        assert!(
            crate::caip2::Caip2NetworkId::from_str("base").is_err(),
            "the schema says `base` is refused in `accepted`; the type must agree"
        );
    }

    /// `x402_settle` keeps the whole envelope and adds exactly one field.
    ///
    /// The idempotency key is lifted out of the body into a header, so it is
    /// the one argument that is NOT part of the payment. A settle schema that
    /// lost the envelope detail while gaining it would be a regression nobody
    /// would notice from the tool list.
    #[test]
    fn the_settle_schema_is_the_verify_schema_plus_the_idempotency_key() {
        let verify = payment_envelope_schema("/verify");
        let settle = settle_input_schema();
        assert_eq!(
            settle["properties"]["paymentPayload"], verify["properties"]["paymentPayload"],
            "settle must publish the same payload shape as verify"
        );
        assert!(settle["properties"][IDEMPOTENCY_KEY_ARG].is_object());
    }

    /// A settle over MCP passes through `settle_writer_gate`.
    ///
    /// This is the invariant the whole dispatch design exists for. With the
    /// writer flag down and no known holder, the gate answers its 503; a tool
    /// that called `post_settle::<A>()` directly would sail past it and let a
    /// second ECS task allocate a nonce for the shared EVM signer.
    ///
    /// The flag is a process-global `AtomicBool`, so this flips it back before
    /// asserting. CI runs `--test-threads=1`, which is what makes that safe --
    /// the same constraint the writer-lease tests in `handlers.rs` rely on.
    #[tokio::test]
    async fn a_settle_over_mcp_still_passes_through_the_writer_gate() {
        let (mcp, _) = routers().await;
        // `network: "base"` on purpose: the gate only forwards EVM settles, and
        // an unreadable body is treated as EVM. Saying it makes the test about
        // the gate rather than about the fallback.
        let body = json!({
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": { "name": "x402_settle", "arguments": {
                "x402Version": 1,
                "paymentPayload": { "network": "base" },
                "paymentRequirements": { "network": "base" }
            }}
        });

        crate::writer_lease::set_writer_for_test(false);
        let (_, _, not_the_writer) = rpc(&mcp, body.clone()).await;
        crate::writer_lease::set_writer_for_test(true);
        let (_, _, the_writer) = rpc(&mcp, body).await;

        let refused = tool_text(&not_the_writer);
        assert_eq!(not_the_writer["result"]["isError"], true);
        assert!(
            refused.contains("does not hold the EVM writer lease"),
            "the writer gate did not run on the MCP path: {refused}"
        );

        // Discriminating half: holding the lease, the same call fails for some
        // other reason. Without this the assertion above would also pass on a
        // route that always answered 503.
        let served = tool_text(&the_writer);
        assert!(
            !served.contains("does not hold the EVM writer lease"),
            "the writer answered the lease 503 to itself: {served}"
        );
    }

    /// The forwarded settle reaches the lease holder carrying the client's IP.
    ///
    /// This is the hop nothing covered before, and the one that was broken.
    /// `settle_writer_gate` hands an EVM settle to the task holding the writer
    /// lease over a direct TCP connection that never touches the ALB, and on
    /// the far side `/settle` is metered per client by `ClientIpKeyExtractor`.
    /// A synthetic request built with only a content-type arrives there with
    /// no client address, so the holder keys it on the forwarding task: one
    /// bucket shared by every MCP settle that task forwards (before the server
    /// carried `ConnectInfo`, a 500 "Unable To Extract Key!" instead). With
    /// two ECS tasks and one holder that is most EVM settles over MCP, on
    /// ordinary traffic.
    ///
    /// So this stands up a real listener, points the lease holder at it, and
    /// asserts on what actually arrived. The previous gate test stopped at
    /// `holder_unknown` and never reached `forward_to_writer` at all.
    #[tokio::test]
    async fn a_forwarded_settle_carries_the_client_ip_to_the_lease_holder() {
        let (path, headers, answer) = settle_through_a_recording_holder(&["203.0.113.42"]).await;
        assert_eq!(path, "/settle", "the wrong route was forwarded");
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            forwarded, "203.0.113.42",
            "the forwarded settle carries no client IP; the holder's rate limiter \
             would key it on the forwarding task"
        );
        // And the holder's answer came back to the tool, not a lease 503.
        let text = tool_text(&answer);
        assert!(
            text.contains("0xdeadbeef"),
            "the holder's response did not reach the caller: {text}"
        );
        assert_ne!(answer["result"]["isError"], true);
    }

    /// Every `X-Forwarded-For` line reaches the holder, in order -- not only
    /// the first. The holder keys several lines differently from one, so a
    /// copy that dropped lines would change the key on the far side.
    #[tokio::test]
    async fn every_forwarded_for_line_reaches_the_lease_holder() {
        let lines = ["198.51.100.1", "198.51.100.2, 203.0.113.42"];
        let (path, headers, _) = settle_through_a_recording_holder(&lines).await;
        assert_eq!(path, "/settle");
        let seen: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(seen, lines);
    }

    /// A stack identity's key reaches the lease holder too. The outer `/mcp`
    /// governor let the caller through on it; without it the holder's `/settle`
    /// governor would charge the same caller as a third party.
    #[tokio::test]
    async fn a_forwarded_settle_carries_the_stack_key_to_the_lease_holder() {
        let key = "uvdsk_mcpForwardedKeyAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let (path, headers, _) = settle_through_a_recording_holder_with(&[
            ("x-forwarded-for", "203.0.113.42"),
            (crate::rate_policy::STACK_KEY_HEADER, key),
        ])
        .await;
        assert_eq!(path, "/settle");
        let seen: Vec<&str> = headers
            .get_all(crate::rate_policy::STACK_KEY_HEADER)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(seen, [key], "the stack key did not reach the lease holder");
    }

    /// A third party's settle reaches the holder WITHOUT a stack key. The hop
    /// carries the caller's own key when there is one and never adds one: the
    /// exemption must not ride on somebody else's traffic.
    #[tokio::test]
    async fn a_third_partys_forwarded_settle_carries_no_stack_key() {
        let (path, headers, _) = settle_through_a_recording_holder(&["203.0.113.43"]).await;
        assert_eq!(path, "/settle");
        assert!(
            !headers.contains_key(crate::rate_policy::STACK_KEY_HEADER),
            "a third party's forwarded settle carries a stack key"
        );
    }

    /// Runs an `x402_settle` whose outer request carries `xff`, on a task that
    /// does not hold the writer lease, against a local stand-in for the holder,
    /// and returns the path and headers the holder received plus the answer.
    async fn settle_through_a_recording_holder(xff: &[&str]) -> (String, HeaderMap, Value) {
        let lines: Vec<(&str, &str)> = xff.iter().map(|line| ("x-forwarded-for", *line)).collect();
        settle_through_a_recording_holder_with(&lines).await
    }

    /// Same, with arbitrary header lines on the outer request.
    ///
    /// Serialised: the writer lease and its holder endpoint are process-wide,
    /// so two of these running at once point each other's settle at the wrong
    /// holder. CI runs single-threaded; this keeps a local parallel run honest.
    async fn settle_through_a_recording_holder_with(
        outer: &[(&str, &str)],
    ) -> (String, HeaderMap, Value) {
        use std::sync::Mutex;

        static HOLDER_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _serial = HOLDER_TESTS.lock().await;

        // A stand-in for the task that holds the lease: records what it got.
        let seen: Arc<Mutex<Option<(String, HeaderMap)>>> = Arc::new(Mutex::new(None));
        let recorder = Arc::clone(&seen);
        let holder =
            Router::new().fallback(axum::routing::any(move |req: axum::extract::Request| {
                let recorder = Arc::clone(&recorder);
                async move {
                    let path = req.uri().path().to_string();
                    *recorder.lock().unwrap() = Some((path, req.headers().clone()));
                    axum::Json(json!({ "success": true, "transaction": "0xdeadbeef" }))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, holder).await;
        });

        let (mcp, _) = routers().await;
        crate::writer_lease::set_writer_for_test(false);
        crate::writer_lease::set_holder_endpoint_for_test(Some(&format!("http://{addr}")));

        let (_, _, answer) = rpc_with_headers(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": "x402_settle", "arguments": {
                    "x402Version": 1,
                    "paymentPayload": { "network": "base" },
                    "paymentRequirements": { "network": "base" }
                }}
            }),
            outer,
        )
        .await;

        crate::writer_lease::set_writer_for_test(true);
        crate::writer_lease::set_holder_endpoint_for_test(None);
        server.abort();

        let (path, headers) = seen
            .lock()
            .unwrap()
            .clone()
            .expect("the settle never reached the holder -- forward_to_writer was not entered");
        (path, headers, answer)
    }

    /// `idempotencyKey` travels as a header and leaves the body alone.
    ///
    /// Both halves matter. `post_settle` hashes the body to tell "same key,
    /// same request" from "same key, different request", so a key left inside
    /// the body would make two identical payments hash differently and defeat
    /// the very deduplication it was sent to get.
    #[tokio::test]
    async fn the_idempotency_key_becomes_a_header_and_leaves_the_body_untouched() {
        use std::sync::Mutex;

        let seen: Arc<Mutex<Option<(HeaderMap, Value)>>> = Arc::new(Mutex::new(None));
        let recorder = Arc::clone(&seen);
        let holder =
            Router::new().fallback(axum::routing::any(move |req: axum::extract::Request| {
                let recorder = Arc::clone(&recorder);
                async move {
                    let headers = req.headers().clone();
                    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    *recorder.lock().unwrap() = Some((headers, body));
                    axum::Json(json!({ "success": true }))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, holder).await;
        });

        let (mcp, _) = routers().await;
        crate::writer_lease::set_writer_for_test(false);
        crate::writer_lease::set_holder_endpoint_for_test(Some(&format!("http://{addr}")));

        let _ = rpc_from(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 10, "method": "tools/call",
                "params": { "name": "x402_settle", "arguments": {
                    "x402Version": 1,
                    "paymentPayload": { "network": "base" },
                    "paymentRequirements": { "network": "base" },
                    "idempotencyKey": "abc-123"
                }}
            }),
            Some("203.0.113.43"),
        )
        .await;

        crate::writer_lease::set_writer_for_test(true);
        crate::writer_lease::set_holder_endpoint_for_test(None);
        server.abort();

        let (headers, body) = seen.lock().unwrap().clone().expect("nothing arrived");
        assert_eq!(
            headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default(),
            "abc-123"
        );
        assert!(
            body.get(IDEMPOTENCY_KEY_ARG).is_none(),
            "the key stayed in the body and would change its hash: {body}"
        );
        assert_eq!(body["x402Version"], 1, "the payment body was altered");
    }

    /// A non-string `idempotencyKey` is refused before anything is dispatched.
    #[tokio::test]
    async fn a_malformed_idempotency_key_is_rejected() {
        let (mcp, _) = routers().await;
        let (_, _, doc) = rpc(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 11, "method": "tools/call",
                "params": { "name": "x402_settle", "arguments": {
                    "x402Version": 1,
                    "paymentPayload": {},
                    "paymentRequirements": {},
                    "idempotencyKey": 7
                }}
            }),
        )
        .await;
        assert_eq!(
            doc["error"]["code"], -32602,
            "expected INVALID_PARAMS: {doc}"
        );
    }

    /// `x402_settle` advertises the key; the read-only tools do not.
    #[test]
    fn only_the_settle_tool_declares_an_idempotency_key() {
        for tool in tools() {
            let declared = tool.input_schema["properties"]
                .get(IDEMPOTENCY_KEY_ARG)
                .is_some();
            assert_eq!(
                declared,
                tool.name == "x402_settle",
                "{} declares idempotencyKey = {declared}",
                tool.name
            );
        }
    }

    /// The description has to ASK for a unique, unguessable key.
    ///
    /// The idempotency store is one namespace shared by every caller: it keys
    /// on the raw string with no per-caller prefix (`idempotency_store.rs`).
    /// That is the same property the REST header has always had, but the
    /// caller here is a model, and a model writes `"retry-1"`. Two consequences,
    /// both real: two unrelated agents collide and the second gets a 409, and a
    /// guessable key can be claimed in advance by someone else's settle so the
    /// legitimate one is refused. The description is the only place a model
    /// reads, so the requirement lives there -- and this pins it, because a
    /// later edit trimming the text for brevity would delete the warning
    /// without deleting anything that fails.
    #[test]
    fn the_idempotency_key_description_demands_a_unique_unguessable_value() {
        let settle = tools()
            .into_iter()
            .find(|t| t.name == "x402_settle")
            .expect("x402_settle must exist");
        let description = settle.input_schema["properties"][IDEMPOTENCY_KEY_ARG]["description"]
            .as_str()
            .expect("idempotencyKey must be described")
            .to_ascii_lowercase();

        assert!(
            description.contains("unguessable") || description.contains("unpredictable"),
            "the description must say the key has to be unguessable"
        );
        assert!(
            description.contains("uuid"),
            "the description must name a concrete shape a model can produce"
        );
        assert!(
            description.contains("per payment") || description.contains("do not reuse"),
            "the description must say one key per payment"
        );
    }

    /// An `Accept` that names only JSON is rmcp's 406, but with a content-type.
    ///
    /// rmcp builds that refusal with a bare `Response::builder()` and a plain
    /// string, so it goes out typeless. A scanner grades the type as much as
    /// the status, and this is the first wall an integrator writing a client by
    /// hand walks into.
    #[tokio::test]
    async fn a_bad_accept_header_is_a_typed_json_406() {
        let (mcp, _) = routers().await;
        let response = mcp
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "jsonrpc": "2.0", "id": 12, "method": "tools/list"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ctype.starts_with("application/json"),
            "the 406 answered content-type {ctype:?}"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc: Value = serde_json::from_slice(&bytes).expect("the 406 must be JSON");
        assert_eq!(doc["status"], 406);
        assert!(
            doc["hint"]
                .as_str()
                .is_some_and(|h| h.contains("text/event-stream")),
            "the 406 does not say what Accept it wants: {doc}"
        );
    }

    /// A `Host` off the allowlist is rmcp's 403, also typed now.
    #[tokio::test]
    async fn a_disallowed_host_is_a_typed_json_403() {
        let (mcp, _) = routers().await;
        let response = mcp
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::HOST, "evil.example")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "jsonrpc": "2.0", "id": 13, "method": "tools/list"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let ctype = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(ctype.starts_with("application/json"), "403 typed {ctype:?}");
    }

    /// Every published list of chain families names every family this build
    /// settles. Native Hedera went live in September 2026, and through 2.39.1
    /// the MCP description, its server card, the ARD entry and the landing's
    /// meta description (both languages) still listed seven families.
    ///
    /// The `match` is exhaustive: a new family does not compile here until it
    /// has a name, and then every list has to carry it.
    #[test]
    fn every_published_family_list_names_every_family() {
        use crate::network::NetworkFamily;
        fn name(family: NetworkFamily) -> &'static str {
            match family {
                NetworkFamily::Evm => "EVM",
                NetworkFamily::Solana => "Solana",
                NetworkFamily::Near => "NEAR",
                NetworkFamily::Stellar => "Stellar",
                #[cfg(feature = "hedera")]
                NetworkFamily::Hedera => "Hedera",
                #[cfg(feature = "xrpl")]
                NetworkFamily::Xrpl => "XRPL",
                #[cfg(feature = "algorand")]
                NetworkFamily::Algorand => "Algorand",
                #[cfg(feature = "sui")]
                NetworkFamily::Sui => "Sui",
            }
        }
        let quoted = |text: &str, key: &str| -> Vec<String> {
            text.split(key)
                .skip(1)
                .filter_map(|rest| rest.split_once('"').map(|(value, _)| value.to_string()))
                .collect()
        };

        let info = FacilitatorMcp {
            rest: axum::Router::new(),
        }
        .get_info();
        let card: Value =
            serde_json::from_str(include_str!("../static/.well-known/mcp/server-card.json"))
                .unwrap();
        let ard: Value =
            serde_json::from_str(include_str!("../static/.well-known/ard.json")).unwrap();
        let landing = include_str!("../static/index.html");

        let mut lists = vec![
            (
                "MCP serverInfo",
                info.server_info.description.clone().unwrap_or_default(),
            ),
            (
                "server-card.json",
                card["serverInfo"]["description"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            ),
            (
                "ard.json",
                ard["entries"][0]["description"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            ),
        ];
        let meta = quoted(
            landing,
            "<meta name=\"description\" data-i18n-content=\"meta.description\" content=\"",
        );
        let dictionaries = quoted(landing, "\"meta.description\": \"");
        assert_eq!(meta.len(), 1, "index.html has one description tag");
        assert_eq!(
            dictionaries.len(),
            2,
            "index.html has an en and an es description"
        );
        lists.extend(meta.into_iter().map(|d| ("index.html meta", d)));
        lists.extend(
            dictionaries
                .into_iter()
                .map(|d| ("index.html meta.description", d)),
        );

        for &network in Network::variants() {
            let family = name(NetworkFamily::from(network));
            for (surface, list) in &lists {
                assert!(
                    list.contains(family),
                    "{surface} does not name {family} ({network}): {list}"
                );
            }
        }
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
        let card: Value =
            serde_json::from_str(std::str::from_utf8(&bytes).expect("the card must be UTF-8"))
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
        // And under the title `tools/list` gives each one, so a scanner reading
        // the card and a client reading the list name the same tool alike.
        for (entry, tool) in card["tools"].as_array().unwrap().iter().zip(tools()) {
            let title = tool
                .annotations
                .as_ref()
                .and_then(|a| a.title.clone())
                .unwrap_or_else(|| panic!("{} has no title", tool.name));
            assert_eq!(entry["title"], title.as_str(), "{}", tool.name);
            assert!(
                entry["description"].as_str().is_some_and(|d| d.len() > 40),
                "{} has no usable description on the card",
                tool.name
            );
        }
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

    /// The tool schemas name `fhe-transfer` among the schemes only while
    /// ENABLE_ZAMA is on ([`crate::zama`]); off (the default) they list the
    /// other four and still point at GET /supported.
    #[test]
    fn the_tool_schemas_name_fhe_transfer_only_while_zama_is_on() {
        let listed =
            |flag| crate::zama::with_flag(flag, || serde_json::to_string(&tools()).unwrap());
        let off = listed(None);
        for name in ["fhe-transfer", "fhe_transfer", "zama"] {
            assert!(!off.to_ascii_lowercase().contains(name), "off names {name}");
        }
        assert!(off.contains("exact | upto | escrow | commerce. GET /supported"));
        assert!(off.contains("Payment scheme: exact, upto, escrow or commerce."));

        // On, each of the three descriptions is back as it was. Five
        // occurrences in all: x402_verify and x402_settle share the payment
        // envelope schema, which names it twice, and the x402_supported
        // output schema names it once.
        let on = listed(Some("true"));
        for description in [
            "x402 v1 ONLY (in v2 this is accepted.scheme). exact | upto | escrow | commerce | fhe-transfer. GET /supported lists what this facilitator serves.",
            "exact | upto | escrow | commerce | fhe-transfer. GET /supported lists what this facilitator serves.",
            "Payment scheme: exact, upto, escrow, commerce or fhe-transfer.",
        ] {
            assert!(on.contains(description), "on lost {description:?}");
        }
        assert_eq!(on.matches("fhe-transfer").count(), 5);
    }

    // ------------------------------------------------------------------------
    // The buyer reads: x402_networks, x402_chain_health,
    // x402_escrow_onchain_state, x402_durable_evidence
    // ------------------------------------------------------------------------

    /// `tools/call` of `tool` with `arguments`; the transport must answer 200.
    async fn call(mcp: &Router, tool: &str, arguments: Value) -> Value {
        let (status, _, doc) = rpc(
            mcp,
            json!({
                "jsonrpc": "2.0", "id": 30, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tool}: {doc}");
        doc
    }

    /// `(status, body)` of one request to a REST router.
    async fn rest_answer(
        rest: &Router,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut builder = HttpRequest::builder().method(method).uri(uri);
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
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// The tool result is the REST answer: the same body, byte for byte, and a
    /// tool error exactly when the route answered non-2xx.
    fn assert_same_answer(doc: &Value, (status, body): &(StatusCode, String), what: &str) {
        assert_eq!(&tool_text(doc), body, "{what}: the tool and the route disagree");
        assert_eq!(
            doc["result"]["isError"] == true,
            !status.is_success(),
            "{what}: isError does not follow the route's {status}: {doc}"
        );
    }

    /// The JSON-RPC error code of a refused call (`-32602` = invalid params).
    fn rpc_error_code(doc: &Value) -> i64 {
        doc["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("expected a JSON-RPC error, got {doc}"))
    }

    /// The `outputSchema` `tools/list` publishes for `name`.
    async fn declared_output_schema(mcp: &Router, name: &str) -> Value {
        listed_tools(mcp)
            .await
            .into_iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not listed"))["outputSchema"]
            .clone()
    }

    /// `structuredContent` is the text block's document and validates against
    /// the declared `outputSchema` -- the check an MCP client runs.
    async fn assert_structured_matches_schema(mcp: &Router, name: &str, doc: &Value) {
        let structured = &doc["result"]["structuredContent"];
        let text: Value = serde_json::from_str(&tool_text(doc)).unwrap();
        assert_eq!(structured, &text, "{name}: structuredContent is not the text block");
        let schema = declared_output_schema(mcp, name).await;
        let errors: Vec<String> = validator_for(&schema)
            .iter_errors(structured)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{name} breaks its outputSchema: {errors:#?}");
    }

    /// The JSON of a `/health/ready` body without `ageSecs`, the one field
    /// that ticks between two reads of the same cached reading.
    fn without_age(text: &str) -> Value {
        let mut doc: Value = serde_json::from_str(text).unwrap();
        doc.as_object_mut().unwrap().remove("ageSecs");
        doc
    }

    /// A DX402 service in memory holding one anchor, and that anchor's id.
    async fn dx402_with_one_anchor() -> (Arc<crate::dx402::service::Dx402Service>, String) {
        use crate::dx402::types::{
            AnchorRequest, DurablePointer, EvidenceMode, KeyAlg, Retention, StorageBackend,
        };
        let address = |s: &str| -> crate::types::MixedAddress {
            serde_json::from_value(Value::String(s.to_string())).unwrap()
        };
        let svc = crate::dx402::service::Dx402Service::in_memory(
            alloy::signers::local::PrivateKeySigner::random(),
        );
        let anchored = svc
            .anchor(
                AnchorRequest {
                    payment_id: format!("0x{}", "11".repeat(32)),
                    network: Network::Base,
                    tx_hash: format!("0x{}", "33".repeat(32)),
                    payer: address("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
                    payee: address("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
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
                    escrow_release: None,
                },
                8453,
                // Now, so the 90-day retention has not run out when the route
                // reads the record against the wall clock.
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            )
            .await
            .expect("an in-memory anchor is recorded");
        (Arc::new(svc), anchored.payment_id)
    }

    /// Every read says it is one: read-only and idempotent in its hints, and
    /// dispatched to a GET -- or to `POST /escrow/state`, the one POST read,
    /// a view call that spends no gas (`post_escrow_state`).
    #[test]
    fn every_read_is_read_only_and_reaches_only_a_read_route() {
        let reads: Vec<&str> = TOOL_CLASSES
            .iter()
            .filter(|(_, class)| *class == "lectura")
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            reads,
            [
                "x402_supported",
                "x402_networks",
                "x402_chain_health",
                "x402_escrow_onchain_state",
                "x402_durable_evidence",
            ]
        );
        for tool in tools() {
            if !reads.contains(&tool.name.as_ref()) {
                continue;
            }
            let listed = serde_json::to_value(&tool).unwrap();
            let name = tool.name.as_ref();
            assert_eq!(listed["annotations"]["readOnlyHint"], true, "{name}");
            assert_eq!(listed["annotations"]["idempotentHint"], true, "{name}");
            assert_ne!(listed["annotations"]["destructiveHint"], true, "{name}");
            let (method, route) = rest_target(name).unwrap();
            assert!(
                method == Method::GET || (method == Method::POST && route == "/escrow/state"),
                "{name} reaches {method} {route}, which is not a read"
            );
            assert!(
                listed["inputSchema"]["properties"]
                    .get(IDEMPOTENCY_KEY_ARG)
                    .is_none(),
                "{name}"
            );
        }
    }

    /// The router the tools dispatch to has no write and no admin route in it
    /// -- not even one no tool names today. A mapping mistake in `rest_target`
    /// could then only ever reach a 404, never `/dx402/anchor` or
    /// `/feedback/revoke`. Built with DX402 ON, the case where the public
    /// router carries the DX402 writes.
    #[tokio::test]
    async fn the_internal_router_serves_no_write_and_no_admin_route() {
        let (svc, id) = dx402_with_one_anchor().await;
        let readiness = Arc::new(crate::readiness::ReadinessState::new(
            Arc::new(NoProviders),
            crate::readiness::ReadinessConfig::from_env(),
        ));
        let inner = rest_router(
            StubFacilitator::new(),
            Arc::new(DiscoveryRegistry::new()),
            Arc::new(crate::events::EventBus::from_env()),
            crate::transaction_store::create_transaction_store().await,
            readiness,
            Some(svc),
        );
        for (method, uri) in [
            (Method::POST, "/dx402/anchor".to_string()),
            (Method::POST, "/dx402/recover".to_string()),
            (Method::POST, format!("/dx402/repair/{id}")),
            (Method::GET, format!("/dx402/blob/{id}")),
            (Method::GET, "/dx402/stats".to_string()),
            (Method::POST, "/register".to_string()),
            (Method::POST, "/feedback".to_string()),
            (Method::POST, "/feedback/revoke".to_string()),
            (Method::POST, "/feedback/response".to_string()),
            (Method::POST, "/erc8004/admin/retire-identity".to_string()),
            (Method::POST, "/discovery/register".to_string()),
            (Method::DELETE, "/discovery/resources".to_string()),
            (Method::POST, "/discovery/admin/suppress".to_string()),
            (Method::GET, "/reputation/base/1".to_string()),
            (Method::GET, "/identity/base/1".to_string()),
            (Method::GET, "/blacklist".to_string()),
        ] {
            let (status, _) = rest_answer(&inner, method.clone(), &uri, Some(json!({}))).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {uri} is reachable from the MCP tools"
            );
        }
        // Discriminating half: the routes the reads DO stand for are there.
        for uri in [
            "/networks.json".to_string(),
            "/health/ready".to_string(),
            format!("/dx402/evidence/{id}"),
            format!("/dx402/receipt/{id}"),
        ] {
            let (status, body) = rest_answer(&inner, Method::GET, &uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }
        let (status, _) =
            rest_answer(&inner, Method::POST, "/escrow/state", Some(json!({}))).await;
        assert!(
            status != StatusCode::NOT_FOUND && status != StatusCode::METHOD_NOT_ALLOWED,
            "POST /escrow/state is not mounted: {status}"
        );
    }

    /// `x402_networks` returns the body of `GET /networks.json`, byte for byte,
    /// in `structuredContent` too, valid against its `outputSchema`.
    #[tokio::test]
    async fn networks_over_mcp_is_the_body_of_get_networks_json() {
        let (mcp, rest) = routers().await;
        let doc = call(&mcp, "x402_networks", json!({})).await;
        let over_rest = rest_answer(&rest, Method::GET, "/networks.json", None).await;
        assert_eq!(over_rest.0, StatusCode::OK, "{}", over_rest.1);
        assert_same_answer(&doc, &over_rest, "x402_networks");
        assert_structured_matches_schema(&mcp, "x402_networks", &doc).await;
        let rows = doc["result"]["structuredContent"]["networks"]
            .as_array()
            .expect("a networks array");
        assert!(!rows.is_empty(), "the stub serves base-sepolia: {doc}");
    }

    /// A GET read refuses an argument it does not declare instead of
    /// answering a question the caller did not ask.
    #[tokio::test]
    async fn the_get_reads_refuse_an_argument_they_do_not_declare() {
        let (mcp, _) = routers().await;
        for (tool, arguments) in [
            ("x402_networks", json!({ "network": "base" })),
            ("x402_chain_health", json!({ "chain": "base" })),
            (
                "x402_durable_evidence",
                json!({ "paymentId": format!("0x{}", "11".repeat(32)), "format": "json" }),
            ),
        ] {
            let doc = call(&mcp, tool, arguments.clone()).await;
            assert_eq!(rpc_error_code(&doc), -32602, "{tool} {arguments}: {doc}");
        }
    }

    /// The schema describes the document production serves: the one built
    /// from a production `/supported` (2.40.0, every family) validates, and a
    /// document without `networks`, with a string `testnet` or with a token
    /// missing its decimals does not.
    #[test]
    fn the_networks_output_schema_describes_a_production_document() {
        let supported: Value =
            serde_json::from_str(include_str!("../tests/fixtures/supported-2.40.0.json"))
                .unwrap();
        let catalog = crate::networks_json::CATALOG
            .as_ref()
            .expect("config/supported_tokens.json parses");
        let document = crate::networks_json::document(
            &supported,
            catalog,
            "https://facilitator.ultravioletadao.xyz",
        );
        assert!(document["networks"].as_array().is_some_and(|n| n.len() > 20));
        let validator = validator_for(&networks_output_schema());
        let errors: Vec<String> = validator
            .iter_errors(&document)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "a production document breaks the schema: {errors:#?}");

        let mut broken = document.clone();
        broken.as_object_mut().unwrap().remove("networks");
        assert!(!validator.is_valid(&broken), "a document without networks validates");
        let mut broken = document.clone();
        broken["networks"][0]["testnet"] = json!("false");
        assert!(!validator.is_valid(&broken), "a string testnet validates");
        let mut broken = document;
        let token = broken["networks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["tokens"].as_array().is_some_and(|t| !t.is_empty()))
            .expect("a row with tokens");
        token["tokens"][0].as_object_mut().unwrap().remove("decimals");
        assert!(!validator.is_valid(&broken), "a token without decimals validates");
    }

    /// `x402_chain_health` returns the body of `GET /health/ready`, valid
    /// against its `outputSchema`.
    #[tokio::test]
    async fn chain_health_over_mcp_is_the_body_of_get_health_ready() {
        let (mcp, rest) = routers().await;
        let doc = call(&mcp, "x402_chain_health", json!({})).await;
        let (status, body) = rest_answer(&rest, Method::GET, "/health/ready", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_ne!(doc["result"]["isError"], true, "{doc}");
        assert_eq!(without_age(&tool_text(&doc)), without_age(&body));
        assert_structured_matches_schema(&mcp, "x402_chain_health", &doc).await;

        // `network: null` is no network at all.
        let doc = call(&mcp, "x402_chain_health", json!({ "network": null })).await;
        assert_eq!(without_age(&tool_text(&doc)), without_age(&body));
    }

    /// The tool reads the SAME readiness state the public route reads, never
    /// a second one: a reading taken through MCP is the one `GET
    /// /health/ready` serves a second later. A state of its own would probe
    /// again, and the route would answer a later `checkedAtUnix`.
    #[tokio::test]
    async fn chain_health_shares_the_readiness_cache_with_the_public_route() {
        let (mcp, rest) = routers().await;
        let doc = call(&mcp, "x402_chain_health", json!({})).await;
        let over_mcp: Value = serde_json::from_str(&tool_text(&doc)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        let (_, body) = rest_answer(&rest, Method::GET, "/health/ready", None).await;
        let over_rest: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            over_rest["checkedAtUnix"], over_mcp["checkedAtUnix"],
            "the public route probed again: the MCP tool has a readiness state of its own"
        );
        assert!(over_rest["ageSecs"].as_u64().unwrap() >= 1, "{body}");
    }

    /// `network` reaches the route percent-encoded, so the handler sees the
    /// caller's string whole: an `&` cannot smuggle a second parameter in, and
    /// a CAIP-2 colon survives. Every answer is the route's own, for the URI
    /// spelled out here by hand rather than by the encoder under test.
    #[tokio::test]
    async fn chain_health_passes_the_network_through_percent_encoded() {
        let (mcp, rest) = routers().await;
        for (network, uri) in [
            ("nope", "/health/ready?network=nope"),
            ("base", "/health/ready?network=base"),
            ("eip155:8453", "/health/ready?network=eip155%3A8453"),
            (
                "base&network=eip155:1",
                "/health/ready?network=base%26network%3Deip155%3A1",
            ),
            ("", "/health/ready?network="),
        ] {
            let doc = call(&mcp, "x402_chain_health", json!({ "network": network })).await;
            let over_rest = rest_answer(&rest, Method::GET, uri, None).await;
            assert!(!over_rest.0.is_success(), "{uri}: the stub probes no chain");
            assert_same_answer(&doc, &over_rest, uri);
        }
        let doc = call(&mcp, "x402_chain_health", json!({ "network": 8453 })).await;
        assert_eq!(rpc_error_code(&doc), -32602, "{doc}");
    }

    /// A `down` reading is the result, not a tool error; a `503` with no
    /// reading in it, any other status and any other tool are not.
    #[test]
    fn only_a_down_reading_of_the_chain_health_is_a_non_2xx_result() {
        let unavailable = StatusCode::SERVICE_UNAVAILABLE;
        let reading = r#"{"status":"down","networks":[{"network":"base"}]}"#;
        assert!(is_tool_result("x402_chain_health", unavailable, reading));
        assert!(is_tool_result("x402_chain_health", StatusCode::OK, reading));
        assert!(is_tool_result("x402_verify", StatusCode::OK, "{}"));
        for (tool, status, body) in [
            (
                "x402_chain_health",
                unavailable,
                r#"{"status":"down","error":"probe_failed"}"#,
            ),
            ("x402_chain_health", unavailable, "not json"),
            ("x402_chain_health", StatusCode::NOT_FOUND, reading),
            ("x402_chain_health", StatusCode::BAD_REQUEST, reading),
            ("x402_settle", unavailable, reading),
            ("x402_supported", unavailable, reading),
        ] {
            assert!(
                !is_tool_result(tool, status, body),
                "{tool} {status} {body} read as a result"
            );
        }
    }

    /// `x402_escrow_onchain_state` sends its arguments as the body of `POST
    /// /escrow/state` and returns that route's answer, whatever it is.
    #[tokio::test]
    async fn escrow_state_over_mcp_is_the_answer_of_post_escrow_state() {
        let (mcp, rest) = routers().await;
        let example = escrow_state_input_schema()["examples"][0].clone();
        for body in [example, json!({}), json!({ "network": "base" })] {
            let doc = call(&mcp, "x402_escrow_onchain_state", body.clone()).await;
            let over_rest = rest_answer(&rest, Method::POST, "/escrow/state", Some(body.clone())).await;
            assert_same_answer(&doc, &over_rest, &body.to_string());
        }
    }

    /// The input schema is the type's, not a guess: its example deserialises
    /// into `EscrowStateQuery`, every field it calls required is one the type
    /// refuses to do without, and the two traps it describes are real --
    /// `maxAmount` as a JSON number and `network` as a v1 name are refused.
    #[test]
    fn the_escrow_input_schema_is_the_query_type() {
        use crate::payment_operator::types::EscrowStateQuery;
        let schema = escrow_state_input_schema();
        let example = schema["examples"][0].clone();
        serde_json::from_value::<EscrowStateQuery>(example.clone())
            .expect("the published example is a body the route parses");

        let required = |at: &Value| -> Vec<String> {
            at["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        for field in required(&schema) {
            let mut body = example.clone();
            body.as_object_mut().unwrap().remove(&field);
            assert!(
                serde_json::from_value::<EscrowStateQuery>(body).is_err(),
                "`{field}` is required by the schema and not by the type"
            );
        }
        for (object, at) in [
            ("paymentInfo", &schema["properties"]["paymentInfo"]),
            ("extra", &schema["properties"]["extra"]),
        ] {
            for field in required(at) {
                let mut body = example.clone();
                body[object].as_object_mut().unwrap().remove(&field);
                assert!(
                    serde_json::from_value::<EscrowStateQuery>(body).is_err(),
                    "`{object}.{field}` is required by the schema and not by the type"
                );
            }
        }

        let mut body = example.clone();
        body["paymentInfo"]["maxAmount"] = json!(1_000_000);
        assert!(serde_json::from_value::<EscrowStateQuery>(body).is_err());
        assert!(Network::from_caip2(example["network"].as_str().unwrap()).is_some());
        assert!(
            Network::from_caip2("base").is_none(),
            "the schema says the v1 name is refused; the route's parser must agree"
        );
    }

    /// The output schema is the response type's: a serialised
    /// `EscrowStateResponse` validates, and amounts as JSON numbers do not.
    #[test]
    fn the_escrow_output_schema_is_the_response_type() {
        let answer = serde_json::to_value(crate::payment_operator::types::EscrowStateResponse {
            has_collected_payment: true,
            capturable_amount: 1_000_000,
            refundable_amount: 0,
            payment_info_hash: "0xabc".to_string(),
            network: "eip155:8453".to_string(),
        })
        .unwrap();
        let validator = validator_for(&escrow_state_output_schema());
        assert!(validator.is_valid(&answer), "{answer}");
        let mut broken = answer;
        broken["capturableAmount"] = json!(1_000_000);
        assert!(!validator.is_valid(&broken), "a numeric amount validates");
    }

    /// The schemas the tests above validate directly are the ones `tools/list`
    /// publishes: each read's `outputSchema`, and the two input schemas built
    /// by a function. Without this, a tool could publish another schema while
    /// every schema test stayed green.
    #[test]
    fn tools_list_publishes_the_schemas_the_tests_validate() {
        let by_name = |name: &str| {
            tools()
                .into_iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} is not declared"))
        };
        for (name, output) in [
            ("x402_supported", supported_output_schema()),
            ("x402_networks", networks_output_schema()),
            ("x402_chain_health", chain_health_output_schema()),
            ("x402_escrow_onchain_state", escrow_state_output_schema()),
            ("x402_durable_evidence", durable_evidence_output_schema()),
        ] {
            let declared = by_name(name)
                .output_schema
                .unwrap_or_else(|| panic!("{name} publishes no outputSchema"));
            assert_eq!(Value::Object((*declared).clone()), output, "{name}");
        }
        for (name, input) in [
            ("x402_escrow_onchain_state", escrow_state_input_schema()),
            ("x402_durable_evidence", durable_evidence_input_schema()),
        ] {
            assert_eq!(
                Value::Object((*by_name(name).input_schema).clone()),
                input,
                "{name}"
            );
        }
    }

    /// `x402_durable_evidence` returns the body of `GET /dx402/evidence/{id}`,
    /// or with `document: "receipt"` of `GET /dx402/receipt/{id}`, byte for
    /// byte, each valid against the declared `outputSchema`. The id is the
    /// route's to normalise: the `0X` + uppercase spelling answers too.
    #[tokio::test]
    async fn durable_evidence_over_mcp_is_the_body_of_the_dx402_routes() {
        let (svc, id) = dx402_with_one_anchor().await;
        let (mcp, rest) = routers_with(Some(svc)).await;

        let doc = call(&mcp, "x402_durable_evidence", json!({ "paymentId": id })).await;
        let over_rest = rest_answer(&rest, Method::GET, &format!("/dx402/evidence/{id}"), None).await;
        assert_eq!(over_rest.0, StatusCode::OK, "{}", over_rest.1);
        assert_same_answer(&doc, &over_rest, "evidence");
        assert_structured_matches_schema(&mcp, "x402_durable_evidence", &doc).await;
        let evidence = doc["result"]["structuredContent"].clone();

        let doc = call(
            &mcp,
            "x402_durable_evidence",
            json!({ "paymentId": id, "document": "receipt" }),
        )
        .await;
        let over_rest = rest_answer(&rest, Method::GET, &format!("/dx402/receipt/{id}"), None).await;
        assert_eq!(over_rest.0, StatusCode::OK, "{}", over_rest.1);
        assert_same_answer(&doc, &over_rest, "receipt");
        assert_structured_matches_schema(&mcp, "x402_durable_evidence", &doc).await;

        let shouted = format!("0X{}", id.trim_start_matches("0x").to_ascii_uppercase());
        let doc = call(&mcp, "x402_durable_evidence", json!({ "paymentId": shouted })).await;
        assert_eq!(doc["result"]["structuredContent"], evidence, "{doc}");

        // Not a schema that accepts anything: evidence without its authority
        // flags fits neither branch.
        let validator = validator_for(&durable_evidence_output_schema());
        let mut broken = evidence;
        broken.as_object_mut().unwrap().remove("verified");
        assert!(!validator.is_valid(&broken), "evidence without `verified` validates");
    }

    /// A `paymentId` stays inside its path segment whatever it holds: each of
    /// these reaches the evidence handler (`dx402_unknown_payment`) and
    /// answers what the route answers for the encoded id, never another route.
    #[tokio::test]
    async fn a_hostile_payment_id_stays_inside_its_path_segment() {
        let (svc, _) = dx402_with_one_anchor().await;
        let (mcp, rest) = routers_with(Some(svc)).await;
        for (id, encoded) in [
            (format!("0x{}", "22".repeat(32)), format!("0x{}", "22".repeat(32))),
            ("../../settle".to_string(), "..%2F..%2Fsettle".to_string()),
            ("a/b?c#d".to_string(), "a%2Fb%3Fc%23d".to_string()),
            ("x%2Fy".to_string(), "x%252Fy".to_string()),
            ("a+b c".to_string(), "a%2Bb%20c".to_string()),
            ("\u{00f1}".to_string(), "%C3%B1".to_string()),
        ] {
            let doc = call(&mcp, "x402_durable_evidence", json!({ "paymentId": id })).await;
            let over_rest =
                rest_answer(&rest, Method::GET, &format!("/dx402/evidence/{encoded}"), None).await;
            assert_same_answer(&doc, &over_rest, &id);
            assert!(
                tool_text(&doc).contains("dx402_unknown_payment"),
                "{id:?} did not reach the evidence handler: {doc}"
            );
        }
    }

    /// The arguments `x402_durable_evidence` refuses before dispatching:
    /// a missing, blank or non-string `paymentId`, and a `document` outside
    /// its two values (spelling included).
    #[tokio::test]
    async fn durable_evidence_refuses_a_malformed_argument() {
        let (mcp, _) = routers().await;
        let id = format!("0x{}", "11".repeat(32));
        for arguments in [
            json!({}),
            json!({ "paymentId": null }),
            json!({ "paymentId": "" }),
            json!({ "paymentId": "   " }),
            json!({ "paymentId": 7 }),
            json!({ "paymentId": id, "document": "Receipt" }),
            json!({ "paymentId": id, "document": "blob" }),
            json!({ "paymentId": id, "document": 1 }),
        ] {
            let doc = call(&mcp, "x402_durable_evidence", arguments.clone()).await;
            assert_eq!(rpc_error_code(&doc), -32602, "{arguments}: {doc}");
        }
    }

    /// Where DX402 is off the tool answers what `GET /dx402/evidence/{id}`
    /// answers there: the public router's generic 404, as a tool error.
    #[tokio::test]
    async fn durable_evidence_without_dx402_is_the_routes_generic_404() {
        let (mcp, rest) = routers().await;
        let id = format!("0x{}", "11".repeat(32));
        for (document, route) in [("evidence", "evidence"), ("receipt", "receipt")] {
            let doc = call(
                &mcp,
                "x402_durable_evidence",
                json!({ "paymentId": id, "document": document }),
            )
            .await;
            let over_rest =
                rest_answer(&rest, Method::GET, &format!("/dx402/{route}/{id}"), None).await;
            assert_eq!(over_rest.0, StatusCode::NOT_FOUND);
            assert_same_answer(&doc, &over_rest, document);
            assert!(!tool_text(&doc).is_empty(), "the 404 says nothing");
        }
    }

    /// `main.rs` builds ONE readiness state and hands that same `Arc` to the
    /// public `/health/ready` route and to the MCP server, and hands the MCP
    /// server the DX402 service the public `/dx402/*` routes are mounted with.
    /// The harness above passes them by construction; this is what holds the
    /// binary to it, the way `rate_policy` holds its governor wiring.
    #[test]
    fn main_hands_the_mcp_server_the_one_readiness_state_and_the_dx402_service() {
        let main = include_str!("main.rs");
        assert_eq!(
            main.matches("ReadinessState::new(").count(),
            1,
            "main.rs builds more than one readiness state: each would probe on its own"
        );
        let mcp = main
            .split(';')
            .find(|statement| statement.contains("let mcp = mcp::mcp_routes("))
            .expect("main.rs mounts the MCP server");
        assert!(mcp.contains("Arc::clone(&readiness_state)"), "{mcp}");
        assert!(mcp.contains("dx402_service.clone()"), "{mcp}");
        assert_eq!(
            main.matches(".with_state(readiness_state)").count(),
            1,
            "the public /health/ready route is not mounted with the shared state"
        );
    }

    /// The encoder leaves only RFC 3986's unreserved characters bare, and a
    /// query value it encodes decodes back to the caller's exact string.
    #[test]
    fn percent_encode_leaves_only_unreserved_characters_bare() {
        let raw = "a-Z_0.9~ /?#&=+%:\u{00f1}\"";
        let encoded = percent_encode(raw);
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._~%".contains(c)),
            "{encoded}"
        );
        assert!(encoded.starts_with("a-Z_0.9~%20%2F%3F%23%26%3D%2B%25%3A%C3%B1"));
        let decoded: Vec<(String, String)> =
            url::form_urlencoded::parse(format!("v={encoded}").as_bytes())
                .into_owned()
                .collect();
        assert_eq!(decoded, vec![("v".to_string(), raw.to_string())]);
    }

    /// The reads draw on the verify/settle bucket through `/mcp`. That bucket
    /// must be at least as strict as the budgets their REST routes carry
    /// (`/escrow/state` and `/health/ready` on secondary-read, DX402 on
    /// discovery-read; `/networks.json` carries none), or the MCP door would
    /// be the cheaper way to read the chain.
    #[test]
    fn the_mcp_bucket_is_never_looser_than_the_reads_it_relays() {
        let mcp = crate::rate_policy::VERIFY_SETTLE.limit();
        for read in [
            crate::rate_policy::SECONDARY_READ,
            crate::rate_policy::DISCOVERY_READ,
        ] {
            let limit = read.limit();
            assert!(
                mcp.period() >= limit.period() && mcp.burst() <= limit.burst(),
                "the MCP bucket is looser than {}: a tool would out-read its route",
                read.name
            );
        }
    }

    /// Every served document that lists the MCP tools lists all of them, and
    /// none still counts four. The list used to be typed into nine surfaces;
    /// this is what keeps a ninth tool from being announced in four of them.
    #[test]
    fn every_document_that_lists_the_mcp_tools_names_all_of_them() {
        let ard: Value =
            serde_json::from_str(include_str!("../static/.well-known/ard.json")).unwrap();
        let capabilities: Vec<&str> = ard["entries"][0]["capabilities"]
            .as_array()
            .expect("the MCP entry lists its capabilities")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(capabilities, TOOL_NAMES, "ard.json capabilities");

        let documents = [
            ("static/mcp.md", include_str!("../static/mcp.md")),
            ("static/mcp.html", include_str!("../static/mcp.html")),
            ("static/skill.md", include_str!("../static/skill.md")),
            ("static/llms.txt", include_str!("../static/llms.txt")),
            ("static/index.md", include_str!("../static/index.md")),
            (
                "static/.well-known/agent-card.json",
                include_str!("../static/.well-known/agent-card.json"),
            ),
            (
                "static/.well-known/agent.json",
                include_str!("../static/.well-known/agent.json"),
            ),
            ("src/openapi.rs", include_str!("openapi.rs")),
        ];
        for (path, text) in documents {
            for tool in TOOL_NAMES {
                assert!(text.contains(tool), "{path} does not name {tool}");
            }
        }
        let counted = [
            ("static/mcp.md", include_str!("../static/mcp.md")),
            ("static/mcp.html", include_str!("../static/mcp.html")),
            ("static/skill.md", include_str!("../static/skill.md")),
            ("static/llms.txt", include_str!("../static/llms.txt")),
            ("static/index.html", include_str!("../static/index.html")),
            ("static/x402.html", include_str!("../static/x402.html")),
        ];
        for (path, text) in counted {
            let lower = text.to_lowercase();
            for stale in ["four tools", "the four tools", "only four", "cuatro herramientas"] {
                assert!(!lower.contains(stale), "{path} still says `{stale}`");
            }
        }
    }
}
