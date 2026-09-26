//! x402 Facilitator HTTP entrypoint.
//!
//! This binary launches an Axum-based HTTP server that exposes the x402 protocol interface
//! for payment verification and settlement via Ethereum-compatible networks.
//!
//! Endpoints:
//! - `GET /verify` – Supported verification schema
//! - `POST /verify` – Verify a payment payload against requirements
//! - `GET /settle` – Supported settlement schema
//! - `POST /settle` – Settle an accepted payment payload on-chain
//! - `POST /accepts` – Negotiate payment requirements (Faremeter middleware)
//! - `GET /supported` – List supported payment kinds (version/scheme/network)
//!
//! This server includes:
//! - OpenTelemetry tracing via `TraceLayer`
//! - CORS support for cross-origin clients
//! - Ethereum provider cache for per-network RPC routing
//!
//! Environment:
//! - `.env` values loaded at startup
//! - `HOST`, `PORT` control binding address
//! - `OTEL_*` variables enable tracing to systems like Honeycomb

use axum::http::Method;
use axum::{Extension, Router};
use dotenvy::dotenv;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors;
use tower_http::limit::RequestBodyLimitLayer;
use url::Url;

/// Maximum request body size accepted by the facilitator.
///
/// Set conservatively. A legitimate `/verify` or `/settle` payload is well
/// under 16 KiB (payment payload + EIP-712 signature). The pre-existing
/// Axum default of 2 MiB allowed multi-megabyte POSTs to OOM the 2 GB
/// Fargate task before any rate limit could kick in.
///
/// Override via the `MAX_REQUEST_BODY_BYTES` env var if a future integration
/// needs more headroom — keep the floor at 16 KiB.
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

use crate::chain::NetworkProviderOps;
use crate::facilitator::Facilitator;
use crate::facilitator_local::FacilitatorLocal;
use crate::provider_cache::{ProviderCache, ProviderMap};
use crate::sig_down::SigDown;
use crate::telemetry::Telemetry;
use crate::types_v2::{DiscoveryMetadata, DiscoveryResource};

// Compliance module
use x402_compliance::ComplianceCheckerBuilder;

mod blocklist;
mod caip2;
mod chain;
mod chain_identity;
mod client_ip;
mod discovery;
mod discovery_aggregator;
mod discovery_attestation;
mod discovery_config;
mod discovery_crawler;
mod discovery_curation;
mod discovery_health;
mod discovery_owner;
mod discovery_price;
mod discovery_revalidation;
mod discovery_security;
mod discovery_store;
mod discovery_terms;
mod dx402;
mod erc8004;
mod escrow;
mod events;
mod facilitator;
mod facilitator_local;
mod fhe_proxy;
mod from_env;
mod handlers;
mod idempotency_store;
mod interop;
mod receipts;
mod json_depth;
mod lease;
mod mcp;
mod negotiate;
mod network;
mod networks_json;
mod nonce_store;
mod openapi;
mod payment_operator;
mod provider_cache;
mod rate_policy;
mod readiness;
mod redact;
mod sig_down;
mod stuck_tx_monitor;
mod telemetry;
mod timestamp;
mod transaction_store;
mod types;
mod types_v2;
mod upto;
mod version;
mod writer_lease;

use discovery::DiscoveryRegistry;
#[allow(unused_imports)]
use discovery_store::DiscoveryStore;
use discovery_store::S3Store;

/// Initializes the x402 facilitator server.
///
/// - Loads `.env` variables.
/// - Initializes OpenTelemetry tracing.
/// - Connects to Ethereum providers for supported networks.
/// - Starts an Axum HTTP server with the x402 protocol handlers.
///
/// Binds to the address specified by the `HOST` and `PORT` env vars.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env variables
    dotenv().ok();

    // Operator commands run INSTEAD of the server, before anything else
    // starts: no telemetry, no providers, no writer election, no listener.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|command| command == "receipts") {
        std::process::exit(receipts::admin::run(&args[1..]).await);
    }

    let telemetry = Telemetry::new()
        .with_name(env!("CARGO_PKG_NAME"))
        .with_version(version::facilitator_version())
        .register();

    let provider_cache = ProviderCache::from_env().await;
    // Abort if we can't initialise Ethereum providers early
    let provider_cache = match provider_cache {
        Ok(provider_cache) => provider_cache,
        Err(e) => {
            tracing::error!("Failed to create Ethereum providers: {}", e);
            std::process::exit(1);
        }
    };

    // Elect a single EVM writer across overlapping tasks. ECS runs two tasks
    // on every rolling deploy, and the in-process nonce allocator is only
    // sound while one process signs for a given EOA.
    //
    // Awaited, and awaited HERE, before the server binds: this call performs
    // the first election attempt, and a task the ALB can already reach while
    // nobody has decided anything is exactly the state that must never be read
    // as "may sign". A task that loses forwards to the winner; a task that
    // cannot reach the control plane at all does not sign
    // (ENABLE_WRITER_LEASE=false is the break-glass).
    let writer_lease = writer_lease::spawn().await;

    // Elect ONE owner of the periodic discovery work across the cluster.
    //
    // Awaited here, before the aggregation and health tasks are started, for
    // the same reason the writer election is: a task that begins a cycle while
    // nobody has decided anything is a task doing work a peer is also doing.
    // Every task keeps serving `/discovery/*`; only the owner refreshes the
    // catalog, and the others follow the snapshot it publishes.
    let discovery_owner = discovery_owner::spawn().await;

    // Initialize compliance checker (OFAC + blacklist)
    tracing::info!("Initializing compliance checker...");
    let compliance_checker = ComplianceCheckerBuilder::new()
        .with_ofac(true)
        .with_blacklist("config/blacklist.json")
        .build()
        .await;

    let compliance_checker = match compliance_checker {
        Ok(checker) => {
            tracing::info!("Compliance checker initialized successfully");
            Arc::new(checker)
        }
        Err(e) => {
            tracing::error!("Failed to initialize compliance checker: {}", e);
            tracing::error!("This is a critical error. Exiting to prevent sanctions violations.");
            std::process::exit(1);
        }
    };

    // Shared behind an Arc so DX402's anchor gate can read transaction receipts
    // through the same connections the facilitator already opened, instead of
    // building a second set.
    let provider_cache = Arc::new(provider_cache);

    // Watch for the facilitator's own transactions wedged in a node's mempool.
    // Read-only, and it runs on every task rather than only the writer: the
    // symptom is visible from any of them, and a queue nobody is watching is
    // how a Polygon settle stayed broken for six days in September 2026.
    stuck_tx_monitor::spawn(Arc::clone(&provider_cache));

    // Compare each EVM RPC's chain id with the one we sign for. An alert, never
    // a refusal: through 2.39.0 two testnets carried the wrong declared id, and
    // a refusal would have switched both off. Background, so startup waits on
    // no RPC.
    chain_identity::spawn(Arc::clone(&provider_cache));

    // Self-check of the escrow operators declared on Arc: ESCROW() and the v3
    // selectors, at startup and every ten minutes. It only decides what
    // /supported announces and whether NEW authorizations are placed; a failed
    // read never stops the process. Background, so startup waits on no RPC.
    if payment_operator::is_enabled() {
        payment_operator::autoverify::spawn(Arc::clone(&provider_cache));
    }

    let facilitator = FacilitatorLocal::new(Arc::clone(&provider_cache), compliance_checker);
    let axum_state = Arc::new(facilitator);

    // Live traffic stream (GET /events, SSE). Lossy broadcast: an observer can never
    // slow down or fail a payment. Config + kill switch via X402_EVENTS_* env.
    let event_bus = Arc::new(events::EventBus::from_env());
    tracing::info!(
        enabled = event_bus.enabled(),
        max_subscribers = event_bus.max_subscribers(),
        "traffic event stream (GET /events)"
    );

    // Historical index of what we processed. Separate from the live stream:
    // /events is lossy by design, this is queryable after the fact. Failing to
    // reach it is NOT fatal — payments do not depend on being recorded.
    let transaction_store = transaction_store::create_transaction_store().await;
    tracing::info!(
        store = transaction_store.store_type(),
        "transaction history store"
    );

    // DX402 durable-evidence. Off unless explicitly enabled: this is an addition
    // to the payment path, never a gate in front of it, so a facilitator that
    // works today must keep working if this stays unconfigured.
    let dx402_service = dx402::Dx402Service::from_env()
        .await
        .map(|svc| Arc::new(svc.with_providers(Arc::clone(&provider_cache))));

    // Initialize Bazaar discovery registry with optional S3 persistence
    tracing::info!("Initializing Bazaar discovery registry...");
    let discovery_registry = if std::env::var("DISCOVERY_S3_BUCKET").is_ok() {
        // S3 persistence configured
        match S3Store::from_env().await {
            Ok(store) => match DiscoveryRegistry::with_store(store).await {
                Ok(registry) => {
                    tracing::info!("Discovery registry initialized with S3 persistence");
                    Arc::new(registry)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to initialize S3 store, falling back to in-memory: {}",
                        e
                    );
                    Arc::new(DiscoveryRegistry::new())
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Failed to create S3 store, falling back to in-memory: {}",
                    e
                );
                Arc::new(DiscoveryRegistry::new())
            }
        }
    } else {
        // No persistence configured, use in-memory only
        tracing::info!("No DISCOVERY_S3_BUCKET configured, using in-memory registry");
        Arc::new(DiscoveryRegistry::new())
    };
    // `settleable` in a listing is a claim about this process: the networks it
    // has a provider for, which is the map `GET /supported` iterates.
    discovery_registry.set_served_networks(provider_cache.values().map(|p| p.network()));

    // Self-registration: register this facilitator as a discoverable resource
    // Only if FACILITATOR_URL is set (indicates production deployment)
    if let Ok(facilitator_url) = std::env::var("FACILITATOR_URL") {
        match Url::parse(&facilitator_url) {
            Ok(url) => {
                // Get supported networks to include in description
                let supported = axum_state.supported().await;
                let network_count = supported.as_ref().map(|s| s.kinds.len()).unwrap_or(0);

                let facilitator_resource = DiscoveryResource::new(
                    url,
                    "facilitator".to_string(),
                    format!(
                        "Ultravioleta DAO x402 Payment Facilitator - supports {} networks for gasless micropayments",
                        network_count / 2 // Divide by 2 because we list both v1 and v2 (CAIP-2) formats
                    ),
                    vec![], // Facilitators don't require payments, they process them
                ).with_metadata(DiscoveryMetadata {
                    category: Some("payment-facilitator".to_string()),
                    provider: Some("Ultravioleta DAO".to_string()),
                    tags: vec![
                        "x402".to_string(),
                        "facilitator".to_string(),
                        "gasless".to_string(),
                        "micropayments".to_string(),
                        "evm".to_string(),
                        "solana".to_string(),
                    ],
                });

                if let Err(e) = discovery_registry.register(facilitator_resource).await {
                    tracing::warn!("Failed to self-register facilitator: {}", e);
                } else {
                    tracing::info!("Self-registered facilitator at {}", facilitator_url);
                }
            }
            Err(e) => {
                tracing::warn!("Invalid FACILITATOR_URL '{}': {}", facilitator_url, e);
            }
        }
    }

    tracing::info!(
        "Discovery registry initialized (store={}, {} resources)",
        discovery_registry.store_type(),
        discovery_registry.count().await
    );

    // Start background aggregation task if enabled
    // Fetches resources from external facilitators (Coinbase, etc.) every hour
    let aggregation_interval_secs = std::env::var("DISCOVERY_AGGREGATION_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3600); // Default: 1 hour

    let enable_aggregation = std::env::var("DISCOVERY_ENABLE_AGGREGATION")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(true); // Enabled by default

    if enable_aggregation {
        tracing::info!(
            interval_secs = aggregation_interval_secs,
            "Starting discovery aggregation background task"
        );
        let registry_for_aggregation = Arc::clone(&discovery_registry);
        let _aggregation_handle = discovery_aggregator::start_aggregation_task(
            (*registry_for_aggregation).clone(),
            aggregation_interval_secs,
        );
    } else {
        tracing::info!("Discovery aggregation is disabled (DISCOVERY_ENABLE_AGGREGATION=false)");
    }

    // Start background crawl task if enabled (Phase 3)
    // Crawls /.well-known/x402 endpoints from configured seed URLs
    let crawl_interval_secs = std::env::var("DISCOVERY_CRAWL_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(86400); // Default: 24 hours

    let enable_crawler = std::env::var("DISCOVERY_ENABLE_CRAWLER")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false); // Disabled by default (no seed URLs configured)

    if enable_crawler {
        // Parse seed URLs from comma-separated environment variable
        let seed_urls = std::env::var("DISCOVERY_CRAWL_URLS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return None;
                }
                match Url::parse(trimmed) {
                    Ok(url) => Some(discovery_crawler::CrawlTarget::new(url)),
                    Err(e) => {
                        tracing::warn!(url = %trimmed, error = %e, "Invalid crawl URL, skipping");
                        None
                    }
                }
            })
            .collect::<Vec<_>>();

        if seed_urls.is_empty() {
            tracing::info!(
                "Discovery crawler enabled but no valid DISCOVERY_CRAWL_URLS configured"
            );
        } else {
            tracing::info!(
                interval_secs = crawl_interval_secs,
                target_count = seed_urls.len(),
                "Starting discovery crawler background task"
            );
            let registry_for_crawl = Arc::clone(&discovery_registry);
            let _crawl_handle = discovery_crawler::start_crawl_task(
                (*registry_for_crawl).clone(),
                seed_urls,
                crawl_interval_secs,
            );
        }
    } else {
        tracing::info!("Discovery crawler is disabled (DISCOVERY_ENABLE_CRAWLER=false)");
    }

    // Start the Bazaar health prober (WS-B). Probes registered URLs with the
    // SSRF-hardened connector; 402 = alive, dead endpoints are quarantined and
    // hidden from the default listing. Liveness lives in a separate S3 overlay.
    let enable_health = std::env::var("DISCOVERY_ENABLE_HEALTH")
        .map(|v| v != "false" && v != "0")
        .unwrap_or(true);
    if enable_health {
        let health_tick = std::env::var("DISCOVERY_HEALTH_TICK")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        // Global in-flight probe cap, and the per-tick budget it feeds.
        //
        // These were sized for wall-clock ("converge the sweep in under a day")
        // on the assumption that a probe costs almost nothing locally because it
        // spends its time waiting. It does not: every probe is a TLS handshake,
        // and 40 in flight against a budget of `max_rps * tick` = 1200 per
        // minute is a CPU load, not an I/O wait. On a one-vCPU task that is what
        // the 60-100 % bursts EVERY MINUTE were -- measured 19:48Z-20:00Z on
        // 2.21.1, with 20 000 freshly imported resources none of which had ever
        // been probed.
        //
        // 8 in flight and 2/s is 120 probes a tick: a 2 000-resource catalog
        // sweeps in ~17 minutes and then everything backs off to the healthy
        // re-probe cadence of seven days. Slower to converge, and convergence
        // was never the thing under pressure.
        let health_concurrency = std::env::var("DISCOVERY_HEALTH_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(8);
        let health_max_rps = std::env::var("DISCOVERY_HEALTH_MAX_RPS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(2);

        // The cross-replica hand-off for demand-driven revalidation. Same table
        // as the leases and the nonces: same key schema, same TTL attribute, and
        // an IAM statement that already permits the writes. A replica that is
        // not the job owner puts requests here; the owner claims them each tick.
        //
        // Configured only when the control plane is reachable. Without it the
        // queue still works within each replica -- it simply cannot pool demand
        // across the three, which costs a slower refresh and nothing else.
        {
            let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let dynamo = aws_sdk_dynamodb::Client::new(&config);
            discovery_registry
                .revalidation()
                .configure_shared(dynamo, lease::table_name())
                .await;
            tracing::info!(
                table = %lease::table_name(),
                "Revalidation hand-off attached"
            );
        }

        let tracker = discovery_registry.health();
        if let Ok(bucket) = std::env::var("DISCOVERY_S3_BUCKET") {
            let key = std::env::var("DISCOVERY_HEALTH_S3_KEY")
                .unwrap_or_else(|_| "bazaar/health.json".to_string());
            tracker.configure_s3(bucket.clone(), key).await;
            // Observed payment terms get their OWN object, not a column in the
            // catalog. One writer (this prober), out of reach of any import, and
            // a build that predates it neither reads nor writes it -- so a
            // rollback leaves the observations intact instead of stripping them
            // on the next snapshot.
            let terms_key = std::env::var("DISCOVERY_TERMS_S3_KEY")
                .unwrap_or_else(|_| "bazaar/terms.json".to_string());
            discovery_registry
                .terms()
                .configure_s3(bucket, terms_key)
                .await;
        }
        let registry_for_health = Arc::clone(&discovery_registry);
        let _health_handle = discovery_health::start_health_task(
            (*registry_for_health).clone(),
            tracker,
            health_tick,
            health_concurrency,
            health_max_rps,
        );
    } else {
        tracing::info!("Discovery health prober is disabled (DISCOVERY_ENABLE_HEALTH=false)");
    }

    // Follow the catalog the owner publishes.
    //
    // This is the other half of single ownership, and without it the change
    // would be a freshness regression rather than a saving: before, every
    // replica kept its own cache current by running its own aggregation, so
    // taking that away leaves two of three tasks answering from whatever they
    // loaded at boot. A non-owner asks S3 for the object's ETag on this cadence
    // and reloads only when it moved -- which is also strictly fresher than
    // before, where a replica's view was as old as its own last hourly cycle.
    {
        let refresh_secs = std::env::var("DISCOVERY_REFRESH_INTERVAL")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(discovery_owner::DEFAULT_REFRESH_SECS);
        let registry_for_refresh = Arc::clone(&discovery_registry);
        let _refresh_handle = discovery_owner::start_snapshot_refresh_task(
            (*registry_for_refresh).clone(),
            refresh_secs,
        );
    }

    // Start the ERC-8004 attestation task (WS-E). ON-CHAIN writes are gated by
    // ENABLE_BAZAAR_ATTESTATIONS (default OFF, no gas spent); the reputation
    // reader runs regardless so the `verification` field reflects any existing
    // on-chain reputation.
    {
        let attest_config = discovery_attestation::AttestationConfig::from_env();
        let targets: Vec<discovery_attestation::AttestTarget> = discovery_registry
            .curation()
            .attest_targets()
            .into_iter()
            .filter_map(|(label, url, net, agent_id)| {
                <crate::network::Network as std::str::FromStr>::from_str(&net)
                    .ok()
                    .map(|network| discovery_attestation::AttestTarget {
                        label,
                        url,
                        network,
                        agent_id,
                    })
            })
            .collect();
        if targets.is_empty() {
            tracing::info!("No ERC-8004 attestation targets configured; attestation task idle");
        } else {
            let _attest_handle = discovery_attestation::start_attestation_task(
                Arc::clone(&axum_state),
                discovery_registry.health(),
                attest_config,
                targets,
                discovery_registry.reputation(),
                discovery_registry.evidence(),
            );
        }
    }

    // F4: Idempotency-Key cache for /settle retries. Backed by DynamoDB in
    // production (env IDEMPOTENCY_TABLE_NAME) and a no-op store in dev,
    // which keeps the pre-F4 "retry re-runs the settle" behaviour intact
    // for environments without the table provisioned. Stored in a global
    // OnceCell so the generic /settle handler can read it without an
    // Extension layer (see comment in src/idempotency_store.rs).
    let idempotency_store = idempotency_store::create_idempotency_store().await;
    tracing::info!(
        store_type = idempotency_store.store_type(),
        "Idempotency-Key cache initialized"
    );
    idempotency_store::set_global_idempotency_store(idempotency_store);
    receipts::init().await.expect("receipt service configuration must be valid");

    let max_body_bytes = std::env::var("MAX_REQUEST_BODY_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.max(16 * 1024)) // never less than 16 KiB
        .unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
    tracing::info!(max_body_bytes, "HTTP request body limit configured");

    // Closes a 2-week-open question (docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md,
    // "Lo que quedo sin verificar"): `#[tokio::main]` does not pin worker_threads, so tokio sizes
    // the runtime from available_parallelism(), which on Linux reads sched_getaffinity() (the CPU
    // affinity mask) -- NOT the cgroup quota. Whether Fargate's 1024 CPU units resolve to 1 worker
    // (head-of-line blocking risk) or N workers at a fraction of a core each (thrashing risk) was
    // never measured. This makes it a `filter-log-events` away instead of a dedicated investigation.
    tracing::info!(
        workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        "tokio worker threads"
    );

    // Per-IP rate limits. Every budget, its default, its override variables and
    // the reason for its number live in ONE place, `rate_policy` (see
    // `rate_policy::BUDGETS`); `GET /config` publishes the values in force.
    // tower_governor's GCRA replenishes one token every `period` and caps the
    // bucket at `burst`: a period, not a rate.
    //
    // Every budget keys on `ClientIpKeyExtractor` (src/client_ip.rs): the
    // client address the ALB appends to X-Forwarded-For, and only without that
    // header the TCP peer, which is why the server below is built with
    // ConnectInfo. Every governor is mounted through `policy.layer(..)`, which
    // a recognized stack identity (`X-UVD-Stack-Key`) goes around: our own
    // services are never refused by a per-IP budget. What they ARE refused by
    // is the global in-flight ceiling (`admission`, 503, everybody), the body
    // deadline (408, everybody) and the ERC-8004 daily write cap (gas,
    // everybody). The per-address in-flight ceiling (429) is policy: the stack
    // skips it too.
    //
    // The budget is legible before it is hit, from the same numbers that
    // enforce it: a third party's 200 and 429 carry `RateLimit-Policy` and
    // `RateLimit` (draft-ietf-httpapi-ratelimit-headers) next to
    // tower_governor's `x-ratelimit-limit` and `x-ratelimit-remaining`, and
    // `/.well-known/uvd-stack.json` lists every bucket `policy.layer` mounted.
    // A recognized stack identity gets `x-ratelimit-exempt` instead.
    let policy = rate_policy::RatePolicy::from_env();
    let admission = rate_policy::Admission::from_env(&policy);
    for budget in rate_policy::BUDGETS {
        let limit = budget.limit();
        tracing::info!(
            budget = budget.name,
            period_ms = limit.period.as_millis() as u64,
            burst = limit.burst,
            "Rate limit configured"
        );
    }
    let verify_settle_config = rate_policy::config(rate_policy::VERIFY_SETTLE.limit());
    // /discovery/register and the bazaar admin routes; see
    // `rate_policy::DISCOVERY_REGISTER` for why the burst is 250.
    let discovery_register_config = rate_policy::config(rate_policy::DISCOVERY_REGISTER.limit());
    // Bazaar reads, sized for a full-catalog walk.
    let discovery_read_config = rate_policy::config(rate_policy::DISCOVERY_READ.limit());
    // /events: how fast long-lived connections may be opened.
    let events_config = rate_policy::config(rate_policy::EVENTS.limit());
    // /identity reads; the 2026-08-29 incident is in `rate_policy::IDENTITY_READ`.
    let identity_read_config = rate_policy::config(rate_policy::IDENTITY_READ.limit());
    // /reputation, /blacklist, POST /escrow/state, /health/ready, /config and
    // the 404 fallback. Its own bucket, not a share of `discovery_read_config`.
    let secondary_read_config = rate_policy::config(rate_policy::SECONDARY_READ.limit());

    let verify_settle = handlers::verify_settle_routes()
        .with_state(axum_state.clone())
        .layer(policy.layer(&verify_settle_config));

    // The MCP server, under the SAME bucket, not a second one built from the
    // same numbers: a `Bucket` holds one `SharedRateLimiter`, so mounting it
    // twice shares the token bucket. An MCP `x402_settle` and a `POST /settle`
    // from one IP therefore draw on one budget -- which is the point, because
    // they cost the chain the same thing -- and both answer with the same
    // `RateLimit-Policy` name, which is how a client learns it. Mounted on the
    // `mcp` door, so the interop manifest publishes the limit there too.
    let mcp = mcp::mcp_routes(
        axum_state.clone(),
        Arc::clone(&discovery_registry),
        Arc::clone(&event_bus),
        Arc::clone(&transaction_store),
    )
    .layer(policy.layer_on(&verify_settle_config, rate_policy::Door::Mcp));

    let discovery_register = handlers::discovery_register_routes()
        .with_state(Arc::clone(&discovery_registry))
        .layer(policy.layer(&discovery_register_config));

    // ERC-8004 write switch: ENABLE_ERC8004_WRITES=false leaves every ERC-8004 write route
    // (/register, /feedback and /feedback/*) unmounted. Defaults to ON. When ON, the writes sit
    // behind a governor of their own (`handlers::erc8004_write_routes_governed`) and the ones that
    // send a transaction behind a per-network daily limit (`erc8004::daily_cap`).
    let erc8004_writes_enabled = std::env::var("ENABLE_ERC8004_WRITES")
        .map(|v| !(v.eq_ignore_ascii_case("false") || v == "0"))
        .unwrap_or(true);
    if !erc8004_writes_enabled {
        tracing::warn!(
            "ENABLE_ERC8004_WRITES=false: ERC-8004 write endpoints (/register, /feedback, \
             /feedback/revoke, /feedback/response) are DISABLED"
        );
    }

    let mut http_endpoints = Router::new()
        .merge(verify_settle)
        .merge(mcp)
        .merge(
            handlers::identity_read_routes()
                .with_state(axum_state.clone())
                .layer(policy.layer(&identity_read_config)),
        )
        .merge(
            handlers::secondary_read_routes()
                .with_state(axum_state.clone())
                .layer(policy.layer(&secondary_read_config)),
        )
        .merge(handlers::routes().with_state(axum_state.clone()))
        // `/health/ready`: per-chain RPC reachability and signer gas, cached.
        // Its own state, because all it needs is the provider map. NOT for the
        // ALB: `/health` stays the liveness check, or a chain outage would
        // have ECS cycle healthy tasks. Metered like the other on-chain reads:
        // the cache bounds the RPC traffic, the governor bounds who gets to
        // make the task wait for a probe.
        .merge(
            readiness::routes()
                .with_state(Arc::new(readiness::ReadinessState::new(
                    Arc::clone(&provider_cache),
                    readiness::ReadinessConfig::from_env(),
                )))
                .layer(policy.layer(&secondary_read_config)),
        )
        // `GET /config`: the policy in force, for an operator or a stack
        // client checking that its identity is recognized (the response to a
        // recognized key carries `x-ratelimit-exempt`). Computed once: nothing
        // it reports changes while the process runs.
        .merge(
            rate_policy::config_routes(rate_policy::document(
                &policy,
                &admission,
                erc8004_writes_enabled
                    .then(erc8004::daily_cap::global)
                    .as_deref(),
            ))
            .layer(policy.layer(&secondary_read_config)),
        );
    if erc8004_writes_enabled {
        // Read the daily write limits now, so they are logged at startup
        // rather than on the first write.
        let _ = erc8004::daily_cap::global();
        let erc8004_writes =
            handlers::erc8004_write_routes_governed(&policy).with_state(axum_state);
        http_endpoints = http_endpoints.merge(erc8004_writes);
    }
    // Admin curation routes share the strict register governor; they 404 unless
    // BAZAAR_ADMIN_TOKEN is configured.
    let discovery_admin = handlers::discovery_admin_routes()
        .with_state(Arc::clone(&discovery_registry))
        .layer(policy.layer(&discovery_register_config));

    let http_endpoints = http_endpoints
        .merge(discovery_register)
        .merge(discovery_admin)
        .merge(
            handlers::discovery_routes()
                .with_state(Arc::clone(&discovery_registry))
                .layer(policy.layer(&discovery_read_config)),
        )
        .merge(
            handlers::transaction_routes()
                .with_state(Arc::clone(&transaction_store))
                .layer(policy.layer(&discovery_read_config)),
        )
        // The agentic-discovery surfaces (/llms.txt, /.well-known/*, ...).
        // Stateless and unmetered: they are static documents, and a crawler
        // that gets 429 on /llms.txt reports the service as unreachable.
        .merge(handlers::agentic_routes())
        // The human pages: metered, unlike the agentic documents above.
        .merge(handlers::human_page_routes_governed(
            &policy,
            rate_policy::HUMAN_PAGES.limit(),
        ))
        .merge(openapi::swagger_routes())
        .merge(
            handlers::events_routes()
                .with_state(Arc::clone(&event_bus))
                .layer(policy.layer(&events_config)),
        );

    // DX402 durable-evidence. Absent unless ENABLE_DX402=true and the store and
    // signing key are configured; `Dx402Service::from_env` logs precisely why it
    // stayed off rather than falling back to something that only looks durable.
    let http_endpoints = match dx402_service.clone() {
        Some(svc) => http_endpoints.merge(
            dx402::handlers::dx402_routes()
                .with_state(svc)
                .layer(policy.layer(&discovery_read_config)),
        ),
        None => http_endpoints,
    };

    let http_endpoints = http_endpoints
        // The 404 for every path no route claims. Both fallbacks are applied
        // HERE, after the last `.merge()` in the file -- including the
        // conditional DX402 one -- because both are order-sensitive: axum keeps
        // the fallback of the router merged last, and
        // `method_not_allowed_fallback` only reaches the `MethodRouter`s
        // registered at the moment it runs.
        //
        // WHY IT IS UNDER A GOVERNOR
        //     It already was, and it has to stay that way: an unmetered 404 is
        //     a free amplification surface, and path scanning is the traffic
        //     that finds one. What was NOT deliberate is WHICH budget it drew
        //     on. axum's `merge` keeps the fallback of the router merged last
        //     (`(true, true) => use the one from other`), and `.layer()` wraps a
        //     router's default fallback along with its routes -- so until now
        //     every unknown path was silently metered by whichever governed
        //     router happened to be merged last. Measured 2026-09-02: it was
        //     `/events`, so 11 unknown paths from one IP earned a 429.
        //
        //     That is the wrong shape. The events budget exists to bound how
        //     fast long-lived SSE connections can be opened; a 404 is a constant
        //     string. It joins the secondary-read budget instead -- the one
        //     already sized for cheap reads -- so a crawler mapping the surface
        //     gets 404s rather than 429s, which is the entire point of giving
        //     the 404 a body. Reordering the merges above can no longer change
        //     this silently.
        .merge(
            Router::new()
                .fallback(handlers::agent_not_found)
                .layer(policy.layer(&secondary_read_config)),
        )
        // The 405 for a path that exists under a different method. axum still
        // computes and attaches the `Allow` header itself.
        .method_not_allowed_fallback(handlers::method_not_allowed)
        // Share discovery registry with all handlers via Extension for settlement tracking
        .layer(Extension(discovery_registry))
        // ...and the event bus, so post_settle can publish after a settle resolves
        .layer(Extension(event_bus))
        .layer(Extension(transaction_store))
        // Admission (`rate_policy::Admission`): the per-address ceiling (429,
        // third parties), the whole body within its deadline (408, everybody),
        // then the machine's ceiling (503, everybody, stack included) -- taken
        // only once the body is in, so an upload that never finishes holds no
        // slot. Inside the tracing layer so a refusal is logged with its
        // status, outside everything else so nothing below runs for it. It
        // also marks `X-UVD-Stack-Key` sensitive before any other layer sees
        // the request.
        .layer(axum::middleware::from_fn_with_state(
            admission,
            rate_policy::admit,
        ))
        .layer(telemetry.http_tracing())
        // gzip for the documents compiled into the binary, computed once per
        // process -- never a gzip pass per request on the task that settles.
        // Outside the tracing layer, so spans still see the plain response. See
        // `handlers::precompressed_static` for the measurement behind this.
        .layer(axum::middleware::from_fn(handlers::precompressed_static))
        // CORS stays permissive — facilitator is intentionally public.
        // First-party callers: photo2melee, ExecutionMarket, meshrelay, plus arbitrary third
        // parties using the public x402 protocol. Tightening CORS would break consumers.
        .layer(
            cors::CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers(cors::Any)
                .expose_headers(
                    [
                        "idempotent-replayed",
                        "payment-response",
                        "x-payment-response",
                        // A browser client can only pace itself on what it can read.
                        "ratelimit-policy",
                        "ratelimit",
                        "retry-after",
                    ]
                    .map(axum::http::HeaderName::from_static),
                ),
        )
        // Body limit MUST be the last layer applied so it wraps everything below.
        // 64 KiB ceiling on POST bodies — caps memory blow-up from oversized JSON.
        .layer(RequestBodyLimitLayer::new(max_body_bytes));

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8080);

    let addr = SocketAddr::new(host.parse().expect("HOST must be a valid IP address"), port);
    tracing::info!("Starting server at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("Failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        });

    let sig_down = SigDown::try_new()?;
    let axum_cancellation_token = sig_down.cancellation_token();
    let axum_graceful_shutdown = async move { axum_cancellation_token.cancelled().await };
    // With ConnectInfo so a request that carries no X-Forwarded-For -- a local
    // run, a direct connection -- is keyed on its TCP peer by the rate limiter
    // instead of being refused for want of a key.
    axum::serve(
        listener,
        http_endpoints.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(axum_graceful_shutdown)
    .await?;

    // Hand the write lease over explicitly instead of making the successor
    // wait out the TTL: during a rolling deploy the incoming task is already
    // healthy and serving, so a fast handover is the difference between a
    // couple of seconds of refused settles and fifteen.
    if let Some(lease) = writer_lease {
        lease.release().await;
    }

    // Same reasoning for the discovery role: releasing lets the successor pick
    // the periodic work up on its next tick instead of waiting out the TTL.
    if let Some(owner) = discovery_owner {
        owner.release().await;
    }

    Ok(())
}
