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
use axum::Router;
use dotenvy::dotenv;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors;
use url::Url;

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
mod discovery_store;
mod escrow;
mod facilitator;
mod facilitator_local;
mod fhe_proxy;
mod from_env;
mod handlers;
mod network;
mod provider_cache;
mod sig_down;
mod telemetry;
mod timestamp;
mod types;
mod types_v2;

use discovery::DiscoveryRegistry;
use discovery_store::S3Store;
#[allow(unused_imports)]
use discovery_store::DiscoveryStore;

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
            Ok(store) => {
                match DiscoveryRegistry::with_store(store).await {
                    Ok(registry) => {
                        tracing::info!("Discovery registry initialized with S3 persistence");
                        Arc::new(registry)
                    }
                    Err(e) => {
                        tracing::warn!("Failed to initialize S3 store, falling back to in-memory: {}", e);
                        Arc::new(DiscoveryRegistry::new())
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create S3 store, falling back to in-memory: {}", e);
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

    let http_endpoints = Router::new()
        .merge(handlers::routes().with_state(axum_state))
        .merge(handlers::discovery_routes().with_state(discovery_registry))
        .layer(telemetry.http_tracing())
        .layer(
            cors::CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers(cors::Any),
        );

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
