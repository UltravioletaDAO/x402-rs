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

pub fn routes<A>() -> Router<A>
where
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    Router::new()
        .route("/", get(get_root))
        .route("/bazaar", get(get_bazaar))
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

/// How long a proxied write may take before this task gives up on the holder.
///
/// A settle waits for a receipt, so this has to clear `TX_RECEIPT_TIMEOUT_SECS`
/// with room to spare — timing out the hop while the holder is still mining
/// would report a failure for a payment that then lands, which is the one
/// outcome worse than refusing outright.
fn writer_forward_timeout() -> std::time::Duration {
    let receipt = std::env::var("TX_RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);
    std::time::Duration::from_secs(receipt.saturating_add(30))
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
enum AdminAuth {
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
fn admin_auth(headers: &axum::http::HeaderMap, env_var: &str) -> AdminAuth {
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
fn admin_reject(auth: AdminAuth) -> Option<Response<axum::body::Body>> {
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
pub async fn get_root() -> impl IntoResponse {
    let html = include_str!("../static/index.html");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .body(html.to_string())
        .unwrap()
}

/// `GET /events/live`: the live traffic viewer.
///
/// Served from the binary rather than a local file because Chrome and Brave
/// treat `file://` as an opaque origin and block its cross-origin requests
/// regardless of CORS headers — a page opened by double-click could never
/// connect to the stream.
#[instrument(skip_all)]
pub async fn get_events_viewer() -> impl IntoResponse {
    let html = include_str!("../static/events-viewer.html");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .body(html.to_string())
        .unwrap()
}

/// `GET /stats`: aggregated metrics, human-readable.
///
/// Its own page rather than another section of the landing page: the landing
/// page is already a monolith, and metrics are read by someone asking a
/// different question than someone evaluating the service.
#[instrument(skip_all)]
pub async fn get_stats_page() -> impl IntoResponse {
    let html = include_str!("../static/stats.html");
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

/// `GET /bazaar`: Returns the curated Bazaar resource explorer (WS-D).
#[instrument(skip_all)]
pub async fn get_bazaar() -> impl IntoResponse {
    let html = include_str!("../static/bazaar.html");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .body(html.to_string())
        .unwrap()
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
const OWNER_SCAN_MAX_BATCHES: u64 = 64;

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

/// Whether `agent_id` currently exists in the registry.
///
/// `Err` means the node returned no verdict; callers must not read that as
/// "does not exist".
async fn agent_token_exists(
    provider: &crate::chain::evm::InnerProvider,
    registry: alloy::primitives::Address,
    agent_id: u64,
) -> Result<bool, String> {
    let identity = IIdentityRegistry::new(registry, provider.clone());
    match identity
        .ownerOf(alloy::primitives::U256::from(agent_id))
        .call()
        .await
    {
        Ok(_) => Ok(true),
        Err(e) => {
            let msg = format!("{e:?}");
            if is_execution_revert(&msg) {
                Ok(false)
            } else {
                Err(format!("ownerOf({agent_id}) probe was inconclusive: {msg}"))
            }
        }
    }
}

/// Upper bound of the agent-ID range to scan, read straight from the registry.
///
/// `totalSupply()` answers in ONE call what [`probe_max_agent_id`] spends ~20
/// SEQUENTIAL `eth_call`s to discover (11 doublings plus ~9 binary-search steps
/// on a 630-agent registry). Those round trips, not the Multicall3 scan itself,
/// were the entire cost of this endpoint: they put celo at 7.4s average and
/// 12.0s peak while every other route answered in 33ms (measured 2026-08-29).
///
/// ASSUMPTION, and it is NOT a new one: `totalSupply` counts tokens that exist,
/// not the highest ID, so a registry with burned agents has
/// `totalSupply < max_id`. The exponential probe plus binary search this
/// replaces ALREADY assumed IDs run contiguously from 1 -- given a gap, the
/// binary search converges on the gap rather than on the true maximum. Nothing
/// here rests on an assumption the previous code did not already make, and
/// [`resolve_first_token_by_owner`] keeps a fallback to the probe for exactly
/// the case where the assumption does not hold.
///
/// `None` means the registry gave no usable answer (the call failed, or it
/// reported zero), which sends the caller to the probe.
async fn registry_total_supply(
    provider: &crate::chain::evm::InnerProvider,
    registry: alloy::primitives::Address,
) -> Option<u64> {
    let identity = IIdentityRegistry::new(registry, provider.clone());
    match identity.totalSupply().call().await {
        Ok(supply) => {
            let total: u64 = supply.try_into().unwrap_or(0);
            (total > 0).then_some(total)
        }
        Err(e) => {
            debug!(error = ?e, "totalSupply unavailable; falling back to the range probe");
            None
        }
    }
}

/// Highest existing agent ID, found by exponential probe plus binary search.
///
/// Costs ~20 sequential `eth_call`s on a 630-agent registry and grows by one
/// more every time the registry doubles, which is why it is now only the
/// FALLBACK for when `totalSupply()` is unavailable or undercounts.
async fn probe_max_agent_id(
    provider: &crate::chain::evm::InnerProvider,
    registry: alloy::primitives::Address,
) -> Result<u64, String> {
    let mut hi: u64 = 1;
    loop {
        if !agent_token_exists(provider, registry, hi).await? {
            break;
        }
        hi = hi.saturating_mul(2);
        if hi > 1_000_000 {
            break;
        }
    }
    let mut lo: u64 = hi / 2;
    while lo < hi.saturating_sub(1) {
        let mid = lo + (hi - lo) / 2;
        if agent_token_exists(provider, registry, mid).await? {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        // Callers only scan when `balanceOf` is non-zero, so an apparently
        // empty registry contradicts the balance and is not a clean "owns
        // nothing" answer.
        return Err("registry probe found no tokens despite a non-zero balance".to_string());
    }
    Ok(lo)
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
) -> Result<Option<u64>, String> {
    use alloy::providers::bindings::IMulticall3;
    use alloy::providers::MULTICALL3_ADDRESS;
    use alloy::sol_types::SolCall;

    if first > last {
        return Ok(None);
    }

    let batches = (last - first + 1).div_ceil(OWNER_SCAN_BATCH);
    if batches > OWNER_SCAN_MAX_BATCHES {
        return Err(format!(
            "registry too large to scan: {first}..={last} needs {batches} batches \
             (cap {OWNER_SCAN_MAX_BATCHES})"
        ));
    }

    let mut start: u64 = first;
    while start <= last {
        let end = (start + OWNER_SCAN_BATCH - 1).min(last);

        let calls: Vec<IMulticall3::Call3> = (start..=end)
            .map(|id| {
                let calldata = IIdentityRegistry::ownerOfCall {
                    agentId: alloy::primitives::U256::from(id),
                }
                .abi_encode();
                IMulticall3::Call3 {
                    target: registry,
                    allowFailure: true,
                    callData: calldata.into(),
                }
            })
            .collect();

        let aggregate_call = IMulticall3::aggregate3Call { calls };
        let encoded = aggregate_call.abi_encode();
        let tx = alloy::rpc::types::TransactionRequest::default()
            .to(MULTICALL3_ADDRESS)
            .input(alloy::rpc::types::TransactionInput::new(encoded.into()));

        let raw_result = provider
            .call(tx)
            .await
            .map_err(|e| format!("Multicall3 batch {start}..={end} failed: {e}"))?;

        // Decode aggregate3 return: Result[] where Result = (bool success, bytes returnData)
        let results = IMulticall3::aggregate3Call::abi_decode_returns(&raw_result)
            .map_err(|e| format!("Failed to decode multicall results: {e}"))?;

        // Find the first token in this batch whose owner matches target.
        for (i, result) in results.iter().enumerate() {
            if !result.success || result.returnData.len() < 32 {
                continue;
            }
            // ownerOf returns abi-encoded address (32 bytes, left-padded)
            let owner = alloy::primitives::Address::from_slice(&result.returnData[12..32]);
            if owner == target {
                return Ok(Some(start + i as u64));
            }
        }

        start = end + 1;
    }

    Ok(None)
}

/// Resolve the first (lowest) token ID owned by `target` in an ERC-721 contract.
///
/// Returns `Ok(Some(id))` on a match, `Ok(None)` when the registry was scanned
/// cleanly and holds no token for `target`, and `Err` when the scan could not
/// reach a verdict (RPC failure, registry too large). The three outcomes are
/// deliberately distinct: callers must not treat an unreachable RPC as proof
/// that an address owns nothing.
///
/// Strategy:
/// 1. Ask the registry its size with `totalSupply()` -- one call
/// 2. Scan `ownerOf(1..=max)` in bounded Multicall3 batches, stopping at the
///    first match -- one batch covers the whole registry on small chains
/// 3. If that finds nothing, re-derive the bound with the expensive probe and
///    scan only what the first pass could not see
/// 4. Cache the hit, since a cold scan is expensive and rarely changes
///
/// Uses `ownerOf` rather than `tokenOfOwnerByIndex` or event scans because it
/// is the only approach that works on every supported chain: the ERC-8004
/// registries are not `ERC721Enumerable` (verified on Base: `supportsInterface`
/// returns false and `tokenOfOwnerByIndex` reverts) and SKALE limits
/// `eth_getLogs` to 2000 blocks.
async fn resolve_first_token_by_owner(
    provider: &crate::chain::evm::InnerProvider,
    network: crate::network::Network,
    registry: alloy::primitives::Address,
    target: alloy::primitives::Address,
) -> Result<Option<u64>, String> {
    // Serve a fresh cached resolution before spending any RPC budget.
    if let Some(entry) = OWNER_LOOKUP_CACHE.get(&(network, registry, target)) {
        let (agent_id, cached_at) = *entry;
        if cached_at.elapsed() < OWNER_LOOKUP_TTL {
            debug!(agent_id, %target, "Owner lookup served from cache");
            return Ok(Some(agent_id));
        }
    }

    // Step 1: upper bound of the range to scan. One call when the registry
    // answers `totalSupply()`, ~20 sequential ones when it does not.
    let supply_bound = registry_total_supply(provider, registry).await;
    let max_id = match supply_bound {
        Some(n) => n,
        None => probe_max_agent_id(provider, registry).await?,
    };

    // Step 2: scan ascending, stopping at the first match so the lowest ID is
    // returned and the common case stays cheap.
    if let Some(agent_id) = scan_range_for_owner(provider, registry, target, 1, max_id).await? {
        OWNER_LOOKUP_CACHE.insert(
            (network, registry, target),
            (agent_id, std::time::Instant::now()),
        );
        return Ok(Some(agent_id));
    }

    // Step 3, belt and braces. Callers only scan with `balanceOf > 0`, so an
    // empty result means the RANGE was wrong, not that the owner holds nothing:
    // `totalSupply` undercounts a registry that has burned agents. Re-derive the
    // bound the expensive way and scan only what the first pass could not see.
    // Skipping this would turn a high-numbered agent into a 404, and callers
    // persist a 404 as "not registered" and stop asking (INC-2026-07-21).
    if supply_bound.is_some() {
        let probed = probe_max_agent_id(provider, registry).await?;
        if probed > max_id {
            debug!(
                total_supply = max_id,
                probed, "totalSupply undercounted the registry; scanning the tail"
            );
            if let Some(agent_id) =
                scan_range_for_owner(provider, registry, target, max_id + 1, probed).await?
            {
                OWNER_LOOKUP_CACHE.insert(
                    (network, registry, target),
                    (agent_id, std::time::Instant::now()),
                );
                return Ok(Some(agent_id));
            }
        }
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
                    // Clean scan, no token attributable to the recipient: the
                    // balance comes from somewhere we cannot map, so minting a
                    // fresh identity is the intended behaviour.
                    Ok(None) => {
                        warn!(
                            network = %network,
                            owner = %target_owner,
                            balance = %balance,
                            "Recipient has balance but no matching token, proceeding with mint"
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
        // The Base registry held ~58.4k agents when the scan broke; it must
        // still be reachable within the batch ceiling.
        let base_registry_size: u64 = 58_400;
        assert!(base_registry_size.div_ceil(OWNER_SCAN_BATCH) <= OWNER_SCAN_MAX_BATCHES);
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
