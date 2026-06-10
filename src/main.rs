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
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_governor::GovernorLayer;
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

use crate::facilitator::Facilitator;
use crate::facilitator_local::FacilitatorLocal;
use crate::provider_cache::ProviderCache;
use crate::sig_down::SigDown;
use crate::telemetry::Telemetry;
use crate::types_v2::{DiscoveryMetadata, DiscoveryResource};

// Compliance module
use x402_compliance::ComplianceCheckerBuilder;

mod blocklist;
mod caip2;
mod chain;
mod discovery;
mod discovery_aggregator;
mod discovery_crawler;
mod discovery_store;
mod erc8004;
mod escrow;
mod facilitator;
mod facilitator_local;
mod fhe_proxy;
mod from_env;
mod handlers;
mod idempotency_store;
mod json_depth;
mod network;
mod nonce_store;
mod openapi;
mod payment_operator;
mod provider_cache;
mod redact;
mod sig_down;
mod telemetry;
mod timestamp;
mod types;
mod types_v2;
mod upto;

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

    let telemetry = Telemetry::new()
        .with_name(env!("CARGO_PKG_NAME"))
        .with_version(env!("CARGO_PKG_VERSION"))
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

    let facilitator = FacilitatorLocal::new(provider_cache, compliance_checker);
    let axum_state = Arc::new(facilitator);

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

    let max_body_bytes = std::env::var("MAX_REQUEST_BODY_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.max(16 * 1024)) // never less than 16 KiB
        .unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
    tracing::info!(max_body_bytes, "HTTP request body limit configured");

    // Per-IP rate limits. tower_governor's GCRA replenishes one token every
    // `per_second` seconds and caps the bucket at `burst_size`, so:
    //   - 1 token every 2s, burst 30  ≈ 30 req/min sustained
    //   - 1 token every 12s, burst 5  ≈ 5 req/min sustained
    // Each /verify or /settle call burns RPC quota against the configured chain
    // providers; /discovery/register triggers DNS + outbound fetches against
    // attacker-supplied URLs (already SSRF-guarded but cheap to spam), so it
    // gets the stricter limit.
    //
    // SmartIpKeyExtractor reads X-Forwarded-For / X-Real-IP / Forwarded
    // headers before falling back to the peer IP — required behind the ALB,
    // where the peer IP is the ALB itself (so the default PeerIpKeyExtractor
    // would either rate-limit ALL clients into one bucket or, without
    // ConnectInfo wired up, fail with "Unable To Extract Key!" 500s on every
    // request).
    let verify_settle_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(30)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("verify/settle governor config must be valid"),
    );
    let discovery_register_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(12)
            .burst_size(5)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("discovery_register governor config must be valid"),
    );

    let verify_settle = handlers::verify_settle_routes()
        .with_state(axum_state.clone())
        .layer(GovernorLayer::new(verify_settle_config));

    let discovery_register = handlers::discovery_register_routes()
        .with_state(Arc::clone(&discovery_registry))
        .layer(GovernorLayer::new(Arc::clone(&discovery_register_config)));

    // ERC-8004 write kill-switch (audit 02): set ENABLE_ERC8004_WRITES=false to disable the
    // gasless reputation/identity write surface entirely (closes the forgery vector). Defaults
    // to ON to preserve existing behavior for operators actively using ERC-8004 writes. When ON,
    // the gas-spending writes sit behind the same strict ~5 req/min governor as discovery_register
    // to cap the gas-treasury drain / bulk reputation-rewrite rate.
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
        .merge(handlers::routes().with_state(axum_state.clone()));
    if erc8004_writes_enabled {
        let erc8004_writes = handlers::erc8004_write_routes()
            .with_state(axum_state)
            .layer(GovernorLayer::new(Arc::clone(&discovery_register_config)));
        http_endpoints = http_endpoints.merge(erc8004_writes);
    }
    let http_endpoints = http_endpoints
        .merge(discovery_register)
        .merge(handlers::discovery_routes().with_state(Arc::clone(&discovery_registry)))
        .merge(openapi::swagger_routes())
        // Share discovery registry with all handlers via Extension for settlement tracking
        .layer(Extension(discovery_registry))
        .layer(telemetry.http_tracing())
        // CORS stays permissive — facilitator is intentionally public.
        // First-party callers: photo2melee, ExecutionMarket, meshrelay, plus arbitrary third
        // parties using the public x402 protocol. Tightening CORS would break consumers.
        .layer(
            cors::CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers(cors::Any),
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
    axum::serve(listener, http_endpoints)
        .with_graceful_shutdown(axum_graceful_shutdown)
        .await?;

    Ok(())
}
