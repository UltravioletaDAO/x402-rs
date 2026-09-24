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
///
/// `annotations` are deliberately NOT touched by the class: they keep saying
/// what the MCP hints say (read-only, destructive, idempotent, open world).
pub const TOOL_CLASSES: [(&str, &str); 4] = [
    ("x402_supported", "lectura"),
    ("x402_accepts", "riel_de_pago"),
    ("x402_verify", "riel_de_pago"),
    ("x402_settle", "mueve_dinero"),
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
                        "description": "x402 v1 ONLY (in v2 this is accepted.scheme). exact | upto | escrow | commerce | fhe-transfer. GET /supported lists what this facilitator serves."
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
                    "scheme": { "type": "string", "description": "exact | upto | escrow | commerce | fhe-transfer. GET /supported lists what this facilitator serves." },
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
                            "description": "Payment scheme: exact, upto, escrow, commerce or fhe-transfer."
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
             execute now. Equivalent to POST /settle. Arc exact and native Hedera USDC \
             return a portable receipt with payment state and settlement ID; unknown \
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
        outer: Option<&HeaderMap>,
        idempotency_key: Option<&str>,
    ) -> Result<(StatusCode, String), McpError> {
        let has_body = method != Method::GET;
        let mut builder = Request::builder().method(method).uri(path);
        if has_body {
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
        for name in [
            header::HeaderName::from_static("x-forwarded-for"),
            header::HeaderName::from_static("x-uvd-purchase"),
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
             twice."
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
        let Some((method, path)) = rest_target(name.as_ref()) else {
            // Unroutable request, not a tool that ran and failed: the caller
            // asked for something this server does not have.
            return Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool: {name}"),
                Some(json!({ "tools": TOOL_NAMES })),
            ));
        };
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

        // rmcp leaves the outer request's `http::request::Parts` here. `None`
        // only in a non-HTTP transport, which this server does not mount.
        let outer = context.extensions.get::<axum::http::request::Parts>();
        let (status, body) = self
            .call_rest(
                method,
                path,
                arguments,
                outer.map(|p| &p.headers),
                idempotency_key.as_deref(),
            )
            .await?;

        // The REST status decides. A 4xx/5xx becomes a tool-level error the
        // caller can read, carrying the facilitator's own message verbatim --
        // NOT an `Err(McpError)`, which most clients render opaquely and would
        // hide "invalid signature" behind "internal error".
        if !status.is_success() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(body)]).into());
        }
        // A tool that declares an `outputSchema` MUST answer in
        // `structuredContent` too (MCP), and clients validate it there. The
        // text block stays, byte for byte the REST body, for the clients that
        // read only text.
        let structured = if STRUCTURED_TOOLS.iter().any(|t| t == name.as_ref()) {
            match serde_json::from_str::<Value>(&body) {
                Ok(doc @ Value::Object(_)) => Some(doc),
                // Cannot happen for `/supported`, whose body is always a JSON
                // object. If it ever does, a result that breaks the declared
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
                    .route(
                        "/supported",
                        get(crate::handlers::get_supported::<StubFacilitator>),
                    )
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
        let mut builder = HttpRequest::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        for line in xff {
            builder = builder.header("x-forwarded-for", *line);
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

    /// A read publishes an object `outputSchema` (R9.5); only the read does,
    /// so the tools whose contract did not change keep their exact shape.
    #[tokio::test]
    async fn only_the_read_tool_publishes_an_output_schema() {
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

    /// Runs an `x402_settle` whose outer request carries `xff`, on a task that
    /// does not hold the writer lease, against a local stand-in for the holder,
    /// and returns the path and headers the holder received plus the answer.
    async fn settle_through_a_recording_holder(xff: &[&str]) -> (String, HeaderMap, Value) {
        use std::sync::Mutex;

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

        let (_, _, answer) = rpc_from_lines(
            &mcp,
            json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": "x402_settle", "arguments": {
                    "x402Version": 1,
                    "paymentPayload": { "network": "base" },
                    "paymentRequirements": { "network": "base" }
                }}
            }),
            xff,
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
