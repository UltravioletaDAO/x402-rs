//! Axum middleware for enforcing [x402](https://www.x402.org) payments on protected routes.
//!
//! This middleware validates incoming `X-Payment` headers using a configured x402 facilitator,
//! and settles valid payments either before or after request execution (configurable).
//!
//! Returns a `402 Payment Required` JSON response if the request lacks a valid payment.
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use axum::{Router, routing::get, Json};
//! use axum::response::IntoResponse;
//! use http::StatusCode;
//! use serde_json::json;
//! use x402_rs::address_evm;
//! use x402_rs::network::{Network, USDCDeployment};
//! use x402_axum::layer::X402Middleware;
//! use x402_axum::price::IntoPriceTag;
//!
//! let x402 = X402Middleware::try_from("https://facilitator.ukstv.me/").unwrap();
//! let usdc = USDCDeployment::by_network(Network::BaseSepolia)
//!     .unwrap()
//!     .pay_to(address_evm!("0x036CbD53842c5426634e7929541eC2318f3dCF7e"));
//!
//! let app: Router = Router::new().route(
//!     "/protected",
//!     get(my_handler).layer(
//!         x402.with_description("Access to /protected")
//!             .with_price_tag(usdc.amount(0.025).unwrap())
//!     ),
//! );
//!
//! async fn my_handler() -> impl IntoResponse {
//!     (StatusCode::OK, Json(json!({ "hello": "world" })))
//! }
//! ```
//!
//! ## Settlement Timing
//!
//! By default, settlement occurs **after** the request is processed. You can change this behavior:
//!
//! - **[`X402Middleware::settle_before_execution`]** - Settle payment **before** request execution.
//!   This prevents issues where failed settlements need retry or authorization expires.
//! - **[`X402Middleware::settle_after_execution`]** - Settle payment **after** request execution (default).
//!   This allows processing the request before committing the payment on-chain.
//!
//! ## Configuration Notes
//!
//! - **[`X402Middleware::with_price_tag`]** sets the assets and amounts accepted for payment.
//! - **[`X402Middleware::with_description`]** and **[`X402Middleware::with_mime_type`]** are optional but help the payer understand what is being paid for.
//! - **[`X402Middleware::with_resource`]** explicitly sets the full URI of the protected resource.
//!   This avoids recomputing [`PaymentRequirements`] on every request and should be preferred when possible.
//! - If `with_resource` is **not** used, the middleware will compute the resource URI dynamically from the request
//!   and a base URL set via **[`X402Middleware::with_base_url`]**.
//! - If no base URL is provided, the default is `http://localhost/` (⚠️ avoid this in production).
//!
//! ## Best Practices (Production)
//!
//! - Use [`X402Middleware::with_resource`] when the full resource URL is known.
//! - Set[`X402Middleware::with_base_url`] to support dynamic resource resolution.
//! - Consider using [`X402Middleware::settle_before_execution`] to avoid settlement failure recovery issues.
//! - ⚠️ Avoid relying on fallback `resource` value in production.

use axum_core::body::Body;

use crate::durable::{
    buffer_body, encode_header, BufferedBody, DurableEvidenceHook, OfferDecision, SettledContext,
};
use axum_core::{
    extract::Request,
    response::{IntoResponse, Response},
};
use http::{HeaderMap, HeaderValue, StatusCode, Uri};
use once_cell::sync::Lazy;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::sync::Arc;
use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::util::BoxCloneSyncService;
use tower::{Layer, Service};
use url::Url;
use x402_rs::dx402::{DurableEvidenceConfig, DurableEvidenceInfo};
use x402_rs::facilitator::Facilitator;
use x402_rs::network::Network;
use x402_rs::types::{
    Base64Bytes, ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, PaymentPayload,
    PaymentRequiredResponse, PaymentRequirements, Scheme, SettleRequest, SettleResponse,
    TokenAmount, VerifyRequest, VerifyResponse, X402Version,
};

#[cfg(feature = "telemetry")]
use tracing::{instrument, Instrument, Level};

use crate::facilitator_client::{FacilitatorClient, FacilitatorClientError};
use crate::price::PriceTag;

/// Middleware layer that enforces x402 payment verification and settlement.
///
/// Wraps an Axum service, intercepts incoming HTTP requests, verifies the payment
/// using the configured facilitator, and performs settlement after a successful response.
/// Adds a `X-Payment-Response` header to the final HTTP response.
#[derive(Clone, Debug)]
pub struct X402Middleware<F> {
    /// The facilitator used to verify and settle payments.
    facilitator: Arc<F>,
    /// Optional description string passed along with payment requirements. Empty string by default.
    description: Option<String>,
    /// Optional MIME type of the protected resource. `application/json` by default.
    mime_type: Option<String>,
    /// Optional resource URL. If not set, it will be derived from a request URI.
    resource: Option<Url>,
    /// Optional base URL for computing full resource URLs if `resource` is not set, see [`X402Middleware::resource`].
    base_url: Option<Url>,
    /// List of price tags accepted for this endpoint.
    price_tag: Vec<PriceTag>,
    /// Offers that carry `durable-evidence`, listed after the plain ones.
    ///
    /// The buyer's opt-in: the same resource, priced separately, with the
    /// declaration on the requirement. Whichever offer gets paid decides
    /// whether the post-hook runs -- see [`crate::durable::OfferDecision`].
    durable_offers: Vec<(PriceTag, DurableEvidenceConfig)>,
    /// The challenge-level `extensions` map, recomputed with the offers.
    extensions: HashMap<String, serde_json::Value>,
    /// Timeout in seconds for payment settlement.
    max_timeout_seconds: u64,
    /// Optional input schema describing the API endpoint's input specification.
    input_schema: Option<serde_json::Value>,
    /// Optional output schema describing the API endpoint's output specification.
    output_schema: Option<serde_json::Value>,
    /// Whether to settle payment before executing the request (true) or after (false, default).
    settle_before_execution: bool,
    /// Cached set of payment offers for this middleware instance.
    ///
    /// This field holds either:
    /// - a fully constructed list of [`PaymentRequirements`] (if [`X402Middleware::with_resource`] was used),
    /// - or a partial list without `resource`, in which case the resource URL will be computed dynamically per request.
    ///   In this case, please add `base_url` via [`X402Middleware::with_base_url`].
    payment_offers: Arc<PaymentOffers>,
    /// Optional DX402 `durable-evidence` post-hook.
    ///
    /// `None` by default: a route that works today must keep working unchanged.
    /// When set, a copy of the response body is sealed to the payer's public key
    /// and anchored after settlement -- see [`crate::durable`].
    durable: Option<Arc<DurableEvidenceHook>>,
}

impl TryFrom<&str> for X402Middleware<FacilitatorClient> {
    type Error = FacilitatorClientError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let facilitator = FacilitatorClient::try_from(value)?;
        Ok(X402Middleware::new(facilitator))
    }
}

impl TryFrom<String> for X402Middleware<FacilitatorClient> {
    type Error = FacilitatorClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        X402Middleware::try_from(value.as_str())
    }
}

impl<F> X402Middleware<F> {
    /// Creates a new middleware instance with a default configuration.
    pub fn new(facilitator: F) -> Self {
        Self {
            facilitator: Arc::new(facilitator),
            description: None,
            mime_type: None,
            resource: None,
            base_url: None,
            max_timeout_seconds: 300,
            price_tag: Vec::new(),
            durable_offers: Vec::new(),
            extensions: HashMap::new(),
            input_schema: None,
            output_schema: None,
            settle_before_execution: false,
            payment_offers: Arc::new(PaymentOffers::Ready(Arc::new(Vec::new()))),
            durable: None,
        }
    }

    /// Enable the DX402 `durable-evidence` post-hook on this route.
    ///
    /// After a successful settlement the response body is sealed to the payer's
    /// own public key, written to `hook`'s sink, and registered with the
    /// facilitator. The buyer receives an `X-Durable-Evidence` header pointing at
    /// it and can decrypt it later with the same key they paid with.
    ///
    /// This never affects whether a payment succeeds. Any failure -- oversized
    /// body, unreachable sink, a smart-contract wallet with no recoverable key --
    /// downgrades to a skip notice in the header and the response is delivered
    /// exactly as it would have been.
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use x402_axum::durable::{DurableConfig, DurableEvidenceHook, HttpPutSink};
    ///
    /// let hook = DurableEvidenceHook::new(
    ///     DurableConfig::default(),
    ///     Arc::new(HttpPutSink::new("https://evidence.example.com")),
    ///     "https://facilitator.ultravioletadao.xyz",
    /// );
    /// let layer = x402.with_durable_evidence(hook);
    /// ```
    pub fn with_durable_evidence(&self, hook: DurableEvidenceHook) -> Self
    where
        F: Clone,
    {
        let mut this = self.clone();
        this.durable = Some(Arc::new(hook));
        this
    }

    /// Returns the configured base URL for x402-protected resources, or `http://localhost/` if not set.
    pub fn base_url(&self) -> Url {
        self.base_url
            .clone()
            .unwrap_or(Url::parse("http://localhost/").unwrap())
    }
}

impl<F> X402Middleware<F>
where
    F: Clone,
{
    /// Sets the description field on all generated payment requirements.
    pub fn with_description(&self, description: &str) -> Self {
        let mut this = self.clone();
        this.description = Some(description.to_string());
        this.recompute_offers()
    }

    /// Sets the MIME type of the protected resource.
    /// This is exposed as a part of [`PaymentRequirements`] passed to the client.
    pub fn with_mime_type(&self, mime: &str) -> Self {
        let mut this = self.clone();
        this.mime_type = Some(mime.to_string());
        this.recompute_offers()
    }

    /// Sets the resource URL directly, avoiding fragile auto-detection from the request.
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_resource(&self, resource: Url) -> Self {
        let mut this = self.clone();
        this.resource = Some(resource);
        this.recompute_offers()
    }

    /// Sets the base URL used to construct resource URLs dynamically.
    ///
    /// Note: If [`with_resource`] is not called, this base URL is combined with
    /// each request's path/query to compute the resource. If not set, defaults to `http://localhost/`.
    ///
    /// ⚠️ In production, prefer calling `with_resource` or setting a precise `base_url` to avoid accidental localhost fallback.
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_base_url(&self, base_url: Url) -> Self {
        let mut this = self.clone();
        this.base_url = Some(base_url);
        this.recompute_offers()
    }

    /// Sets the maximum allowed payment timeout, in seconds.
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_max_timeout_seconds(&self, seconds: u64) -> Self {
        let mut this = self.clone();
        this.max_timeout_seconds = seconds;
        this.recompute_offers()
    }

    /// Replaces all price tags with the provided value(s).
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_price_tag<T: Into<Vec<PriceTag>>>(&self, price_tag: T) -> Self {
        let mut this = self.clone();
        this.price_tag = price_tag.into();
        this.recompute_offers()
    }

    /// Offer the same resource WITH durable evidence, at its own price.
    ///
    /// Adds a second entry to `accepts` that declares `durable-evidence` and
    /// lets the buyer choose. Pair it with [`Self::with_durable_evidence`] for
    /// the hook that does the anchoring; once any offer on the route declares
    /// the extension, paying for a plain offer means the buyer declined and
    /// nothing is anchored for that request.
    ///
    /// The price is the seller's decision, which is the point: evidence has a
    /// cost, and this puts it where the buyer can see it and weigh it.
    ///
    /// ```rust,ignore
    /// let x402 = X402Middleware::try_from("https://facilitator.ultravioletadao.xyz").unwrap()
    ///     .with_price_tag(usdc.amount(0.010).pay_to(seller))          // plain
    ///     .with_durable_offer(usdc.amount(0.012).pay_to(seller),      // +evidence
    ///         DurableEvidenceConfig { retention: Retention::Year1, ..Default::default() })
    ///     .with_durable_evidence(hook);
    /// ```
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_durable_offer<T: Into<PriceTag>>(
        &self,
        price_tag: T,
        config: DurableEvidenceConfig,
    ) -> Self {
        let mut this = self.clone();
        this.durable_offers.push((price_tag.into(), config));
        this.recompute_offers()
    }

    /// Adds new price tags to the existing list, avoiding duplicates.
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn or_price_tag<T: Into<Vec<PriceTag>>>(&self, price_tag: T) -> Self {
        let mut this = self.clone();
        let mut seen: HashSet<PriceTag> = this.price_tag.iter().cloned().collect();
        for tag in price_tag.into() {
            if seen.insert(tag.clone()) {
                this.price_tag.push(tag);
            }
        }
        this.recompute_offers()
    }

    /// Sets the input schema describing the API endpoint's expected inputs.
    ///
    /// The input schema will be embedded in `PaymentRequirements.outputSchema.input`.
    /// This can include information about HTTP method, query parameters, headers, body schema, etc.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use serde_json::json;
    ///
    /// let input_schema = json!({
    ///     "type": "http",
    ///     "method": "GET",
    ///     "discoverable": true,
    ///     "queryParams": {
    ///         "location": {
    ///             "type": "string",
    ///             "description": "City name",
    ///             "required": true
    ///         }
    ///     }
    /// });
    ///
    /// x402.with_input_schema(input_schema)
    /// ```
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_input_schema(&self, schema: serde_json::Value) -> Self {
        let mut this = self.clone();
        this.input_schema = Some(schema);
        this.recompute_offers()
    }

    /// Sets the output schema describing the API endpoint's response format.
    ///
    /// The output schema will be embedded in `PaymentRequirements.outputSchema.output`.
    /// This can include information about the response structure, content type, etc.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use serde_json::json;
    ///
    /// let output_schema = json!({
    ///     "type": "object",
    ///     "properties": {
    ///         "temperature": { "type": "number" },
    ///         "conditions": { "type": "string" }
    ///     }
    /// });
    ///
    /// x402.with_output_schema(output_schema)
    /// ```
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_output_schema(&self, schema: serde_json::Value) -> Self {
        let mut this = self.clone();
        this.output_schema = Some(schema);
        this.recompute_offers()
    }

    /// Enables settlement prior to request execution.
    ///
    /// When enabled, the payment will be settled on-chain **before** the protected
    /// request handler is invoked. This prevents issues where:
    /// - Failed settlements need to be retried via an external process
    /// - Payment authorization expires before final settlement
    ///
    /// When disabled (default), settlement occurs after successful request execution.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use x402_axum::X402Middleware;
    /// use x402_rs::address_evm;
    /// use x402_rs::network::{Network, USDCDeployment};
    /// use x402_axum::IntoPriceTag;
    ///
    /// let x402 = X402Middleware::try_from("https://facilitator.example.com/")
    ///     .unwrap()
    ///     .settle_before_execution()
    ///     .with_price_tag(
    ///         USDCDeployment::by_network(Network::BaseSepolia)
    ///             .unwrap()
    ///             .pay_to(address_evm!("0x036CbD53842c5426634e7929541eC2318f3dCF7e"))
    ///             .amount("0.01")
    ///             .unwrap()
    ///     );
    /// ```
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn settle_before_execution(&self) -> Self {
        let mut this = self.clone();
        this.settle_before_execution = true;
        this
    }

    /// Disables settlement prior to request execution (default behavior).
    ///
    /// When disabled, settlement occurs after successful request execution.
    /// This is the default behavior and allows the application to process
    /// the request before committing the payment on-chain.
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn settle_after_execution(&self) -> Self {
        let mut this = self.clone();
        this.settle_before_execution = false;
        this
    }

    /// Enables dynamic, per-request price computation via a callback.
    ///
    /// The callback receives request headers, URI, and base URL, and returns the
    /// price amount for this request. The resource URL is automatically constructed
    /// from the base URL and request URI, and all partial requirements are updated
    /// with the dynamically computed price.
    ///
    /// This is suitable for dynamic pricing flows where the exact amount depends on
    /// request content, user context, or other runtime factors.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use x402_axum::layer::DynamicPriceCallback;
    ///
    /// let callback: Box<DynamicPriceCallback> = Box::new(move |headers, uri, base_url| {
    ///     Box::pin(async move {
    ///         // Compute price based on request
    ///         Ok(TokenAmount::from(1000000))
    ///     })
    /// });
    ///
    /// x402.with_dynamic_price(callback);
    /// ```
    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn with_dynamic_price<P>(&self, price_callback: P) -> Self
    where
        P: for<'a> Fn(
                &'a HeaderMap,
                &'a Uri,
                &'a Url,
            )
                -> Pin<Box<dyn Future<Output = Result<TokenAmount, X402Error>> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        let mut this = self.clone();
        let base_url = this.base_url();
        let description = this.description.clone().unwrap_or_default();
        let mime_type = this
            .mime_type
            .clone()
            .unwrap_or("application/json".to_string());
        let max_timeout_seconds = this.max_timeout_seconds;
        let partial = this
            .price_tag
            .iter()
            .map(|price_tag| {
                let extra = if let Some(eip712) = price_tag.token.eip712.clone() {
                    Some(json!({ "name": eip712.name, "version": eip712.version }))
                } else {
                    None
                };
                PaymentRequirementsNoResource {
                    scheme: Scheme::Exact,
                    network: price_tag.token.network(),
                    max_amount_required: price_tag.amount,
                    description: description.clone(),
                    mime_type: mime_type.clone(),
                    pay_to: price_tag.pay_to.clone(),
                    max_timeout_seconds,
                    asset: price_tag.token.address(),
                    extra,
                    output_schema: None,
                }
            })
            .collect::<Vec<_>>();
        this.payment_offers = Arc::new(PaymentOffers::DynamicPrice {
            partial,
            base_url,
            price_callback: DynamicPriceFn::new(price_callback),
        });
        this
    }

    fn recompute_offers(mut self) -> Self {
        // The registry-shaped declaration: one object on the challenge, naming
        // the durable offers by their position in `accepts`. `all_offers` puts
        // every plain tag first, so the durable ones sit at the tail.
        self.extensions.remove(x402_rs::dx402::EXTENSION_KEY);
        if let Some((_, config)) = self.durable_offers.first() {
            let first = self.price_tag.len();
            DurableEvidenceInfo {
                config: config.clone(),
                accept_indexes: (first..first + self.durable_offers.len()).collect(),
            }
            .declare(&mut self.extensions);
        }
        let base_url = self.base_url();
        let description = self.description.clone().unwrap_or_default();
        let mime_type = self
            .mime_type
            .clone()
            .unwrap_or("application/json".to_string());
        let max_timeout_seconds = self.max_timeout_seconds;

        // Construct the complete output_schema from input and output schemas
        let complete_output_schema = match (&self.input_schema, &self.output_schema) {
            (Some(input), Some(output)) => Some(json!({
                "input": input,
                "output": output
            })),
            (Some(input), None) => Some(json!({
                "input": input
            })),
            (None, Some(output)) => Some(json!({
                "output": output
            })),
            (None, None) => None,
        };

        let payment_offers = if let Some(resource) = self.resource.clone() {
            let payment_requirements = self
                .all_offers()
                .map(|(price_tag, durable)| {
                    let extra = extra_for(price_tag, durable);
                    PaymentRequirements {
                        scheme: Scheme::Exact,
                        network: price_tag.token.network(),
                        max_amount_required: price_tag.amount,
                        resource: resource.clone(),
                        description: description.clone(),
                        mime_type: mime_type.clone(),
                        pay_to: price_tag.pay_to.clone(),
                        max_timeout_seconds,
                        asset: price_tag.token.address(),
                        extra,
                        output_schema: complete_output_schema.clone(),
                    }
                })
                .collect::<Vec<_>>();
            PaymentOffers::Ready(Arc::new(payment_requirements))
        } else {
            let no_resource = self
                .all_offers()
                .map(|(price_tag, durable)| {
                    let extra = extra_for(price_tag, durable);
                    PaymentRequirementsNoResource {
                        scheme: Scheme::Exact,
                        network: price_tag.token.network(),
                        max_amount_required: price_tag.amount,
                        description: description.clone(),
                        mime_type: mime_type.clone(),
                        pay_to: price_tag.pay_to.clone(),
                        max_timeout_seconds,
                        asset: price_tag.token.address(),
                        extra,
                        output_schema: complete_output_schema.clone(),
                    }
                })
                .collect::<Vec<_>>();
            PaymentOffers::NoResource {
                partial: no_resource,
                base_url,
            }
        };
        self.payment_offers = Arc::new(payment_offers);
        self
    }
}

impl X402Middleware<FacilitatorClient> {
    pub fn facilitator_url(&self) -> &Url {
        self.facilitator.base_url()
    }
}

/// Wraps a cloned inner Axum service and augments it with payment enforcement logic.
#[derive(Clone, Debug)]
pub struct X402MiddlewareService<F> {
    /// Payment facilitator (local or remote)
    facilitator: Arc<F>,
    /// Payment requirements either with static or dynamic resource URLs
    payment_offers: Arc<PaymentOffers>,
    /// Whether to settle payment before executing the request (true) or after (false)
    settle_before_execution: bool,
    /// The inner Axum service being wrapped
    inner: BoxCloneSyncService<Request, Response, Infallible>,
    /// Optional DX402 `durable-evidence` post-hook.
    durable: Option<Arc<DurableEvidenceHook>>,
    extensions: HashMap<String, serde_json::Value>,
}

impl<S, F> Layer<S> for X402Middleware<F>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + Sync + 'static,
    S::Future: Send + 'static,
    F: Facilitator + Clone,
{
    type Service = X402MiddlewareService<F>;

    fn layer(&self, inner: S) -> Self::Service {
        if self.base_url.is_none() && self.resource.is_none() {
            #[cfg(feature = "telemetry")]
            tracing::warn!(
                "X402Middleware base_url is not configured; defaulting to http://localhost/ for resource resolution"
            );
        }
        X402MiddlewareService {
            facilitator: self.facilitator.clone(),
            payment_offers: self.payment_offers.clone(),
            settle_before_execution: self.settle_before_execution,
            inner: BoxCloneSyncService::new(inner),
            durable: self.durable.clone(),
            extensions: self.extensions.clone(),
        }
    }
}

impl<F> Service<Request> for X402MiddlewareService<F>
where
    F: Facilitator + Clone + Send + Sync + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    /// Delegates readiness polling to the wrapped inner service.
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    /// Intercepts the request, injects payment enforcement logic, and forwards to the wrapped service.
    fn call(&mut self, req: Request) -> Self::Future {
        let offers = self.payment_offers.clone();
        let facilitator = self.facilitator.clone();
        let inner = self.inner.clone();
        let settle_before_execution = self.settle_before_execution;
        let durable = self.durable.clone();
        let extensions = self.extensions.clone();
        Box::pin(async move {
            let payment_requirements =
                gather_payment_requirements(offers.as_ref(), req.uri(), req.headers()).await;
            let gate = X402Paygate {
                facilitator,
                payment_requirements,
                settle_before_execution,
                durable,
                extensions,
            };
            gate.call(inner, req).await
        })
    }
}

#[derive(Debug)]
/// Wrapper for producing a `402 Payment Required` response with context.
pub struct X402Error(PaymentRequiredResponse);

impl Display for X402Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "402 Payment Required: {}", self.0)
    }
}

static ERR_PAYMENT_HEADER_REQUIRED: Lazy<String> =
    Lazy::new(|| "X-PAYMENT header is required".to_string());
static ERR_INVALID_PAYMENT_HEADER: Lazy<String> =
    Lazy::new(|| "Invalid or malformed payment header".to_string());
static ERR_NO_PAYMENT_MATCHING: Lazy<String> =
    Lazy::new(|| "Unable to find matching payment requirements".to_string());

/// Middleware application error with detailed context.
///
/// Encapsulates a `402 Payment Required` response that can be returned
/// when payment verification or settlement fails.
impl X402Error {
    pub fn payment_header_required(payment_requirements: Vec<PaymentRequirements>) -> Self {
        let payment_required_response = PaymentRequiredResponse {
            error: ERR_PAYMENT_HEADER_REQUIRED.clone(),
            accepts: payment_requirements,
            x402_version: X402Version::V1,
            extensions: Default::default(),
        };
        Self(payment_required_response)
    }

    pub fn invalid_payment_header(payment_requirements: Vec<PaymentRequirements>) -> Self {
        let payment_required_response = PaymentRequiredResponse {
            error: ERR_INVALID_PAYMENT_HEADER.clone(),
            accepts: payment_requirements,
            x402_version: X402Version::V1,
            extensions: Default::default(),
        };
        Self(payment_required_response)
    }

    pub fn no_payment_matching(payment_requirements: Vec<PaymentRequirements>) -> Self {
        let payment_required_response = PaymentRequiredResponse {
            error: ERR_NO_PAYMENT_MATCHING.clone(),
            accepts: payment_requirements,
            x402_version: X402Version::V1,
            extensions: Default::default(),
        };
        Self(payment_required_response)
    }

    pub fn verification_failed<E2: Display>(
        error: E2,
        payment_requirements: Vec<PaymentRequirements>,
    ) -> Self {
        let payment_required_response = PaymentRequiredResponse {
            error: format!("Verification Failed: {error}"),
            accepts: payment_requirements,
            x402_version: X402Version::V1,
            extensions: Default::default(),
        };
        Self(payment_required_response)
    }

    pub fn settlement_failed<E2: Display>(
        error: E2,
        payment_requirements: Vec<PaymentRequirements>,
    ) -> Self {
        let payment_required_response = PaymentRequiredResponse {
            error: format!("Settlement Failed: {error}"),
            accepts: payment_requirements,
            x402_version: X402Version::V1,
            extensions: Default::default(),
        };
        Self(payment_required_response)
    }
}

impl X402Error {
    /// Attach the challenge-level `extensions` map to this 402.
    pub fn with_extensions(mut self, extensions: HashMap<String, serde_json::Value>) -> Self {
        self.0.extensions = extensions;
        self
    }
}

impl IntoResponse for X402Error {
    fn into_response(self) -> Response {
        let payment_required_response_bytes =
            serde_json::to_vec(&self.0).expect("serialization failed");
        let body = Body::from(payment_required_response_bytes);
        Response::builder()
            .status(StatusCode::PAYMENT_REQUIRED)
            .header("Content-Type", "application/json")
            .body(body)
            .expect("Fail to construct response")
    }
}

/// A service-level helper struct responsible for verifying and settling
/// x402 payments based on request headers and known payment requirements.
pub struct X402Paygate<F> {
    pub facilitator: Arc<F>,
    pub payment_requirements: Arc<Vec<PaymentRequirements>>,
    pub settle_before_execution: bool,
    /// Optional DX402 `durable-evidence` post-hook. `None` leaves the response
    /// path byte-for-byte as it was.
    pub durable: Option<Arc<DurableEvidenceHook>>,
    /// Challenge-level `extensions`, served on every 402 this gate answers.
    pub extensions: HashMap<String, serde_json::Value>,
}

impl<F> X402Paygate<F>
where
    F: Facilitator + Clone + Send + Sync,
{
    /// Parses the `X-Payment` header and returns a decoded [`PaymentPayload`], or constructs a 402 error if missing or malformed as [`X402Error`].
    pub async fn extract_payment_payload(
        &self,
        headers: &HeaderMap,
    ) -> Result<PaymentPayload, X402Error> {
        let payment_header = headers.get("X-Payment");
        let supported = self.facilitator.supported().await.map_err(|e| {
            X402Error(PaymentRequiredResponse {
                x402_version: X402Version::V1,
                error: format!("Unable to retrieve supported payment schemes: {e}"),
                accepts: vec![],
                extensions: Default::default(),
            })
        })?;
        match payment_header {
            None => {
                let requirements = self
                    .payment_requirements
                    .as_ref()
                    .iter()
                    .map(|r| {
                        let mut r = r.clone();
                        let network = r.network;
                        let extra = supported
                            .kinds
                            .iter()
                            .find(|s| s.network == network.to_string())
                            .cloned()
                            .and_then(|s| s.extra);
                        if let Some(extra) = extra {
                            r.extra = Some(json!({
                                "feePayer": extra.fee_payer
                            }));
                            r
                        } else {
                            r
                        }
                    })
                    .collect::<Vec<_>>();
                Err(X402Error::payment_header_required(requirements)
                    .with_extensions(self.extensions.clone()))
            }
            Some(payment_header) => {
                let base64 = Base64Bytes::from(payment_header.as_bytes());
                let payment_payload = PaymentPayload::try_from(base64);
                match payment_payload {
                    Ok(payment_payload) => Ok(payment_payload),
                    Err(_) => Err(X402Error::invalid_payment_header(
                        self.payment_requirements.as_ref().clone(),
                    )),
                }
            }
        }
    }

    /// Finds the payment requirement entry the payload actually pays for.
    fn find_matching_payment_requirements(
        &self,
        payment_payload: &PaymentPayload,
    ) -> Option<PaymentRequirements> {
        match_paid_requirement(&self.payment_requirements, payment_payload)
    }

    /// Verifies the provided payment using the facilitator and known requirements. Returns a [`VerifyRequest`] if the payment is valid.
    #[cfg_attr(
        feature = "telemetry",
        instrument(name = "x402.verify_payment", skip_all, err)
    )]
    pub async fn verify_payment(
        &self,
        payment_payload: PaymentPayload,
    ) -> Result<VerifyRequest, X402Error> {
        let selected = self
            .find_matching_payment_requirements(&payment_payload)
            .ok_or_else(|| {
                X402Error::no_payment_matching(self.payment_requirements.as_ref().clone())
                    .with_extensions(self.extensions.clone())
            })?;
        let verify_request = VerifyRequest {
            x402_version: payment_payload.x402_version,
            payment_payload,
            payment_requirements: selected,
        };
        let verify_response = self
            .facilitator
            .verify(&verify_request)
            .await
            .map_err(|e| {
                X402Error::verification_failed(e, self.payment_requirements.as_ref().clone())
            })?;
        match verify_response {
            VerifyResponse::Valid { .. } => Ok(verify_request),
            VerifyResponse::Invalid { reason, .. } => Err(X402Error::verification_failed(
                reason,
                self.payment_requirements.as_ref().clone(),
            )),
        }
    }

    /// Attempts to settle a verified payment on-chain. Returns [`SettleResponse`] on success or emits a 402 error.
    #[cfg_attr(
        feature = "telemetry",
        instrument(name = "x402.settle_payment", skip_all, err)
    )]
    pub async fn settle_payment(
        &self,
        settle_request: &SettleRequest,
    ) -> Result<SettleResponse, X402Error> {
        let settlement = self.facilitator.settle(settle_request).await.map_err(|e| {
            X402Error::settlement_failed(e, self.payment_requirements.as_ref().clone())
        })?;
        if settlement.success {
            Ok(settlement)
        } else {
            let error_reason = settlement
                .error_reason
                .unwrap_or(FacilitatorErrorReason::InvalidScheme);
            Err(X402Error::settlement_failed(
                error_reason,
                self.payment_requirements.as_ref().clone(),
            ))
        }
    }

    /// Processes an incoming request through the middleware:
    /// determines payment requirements, verifies the payment,
    /// and invokes the inner Axum handler if the payment is valid.
    /// Adds a `X-Payment-Response` header to the response on success.
    pub async fn call<
        ReqBody,
        ResBody,
        S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>>,
    >(
        self,
        inner: S,
        req: http::Request<ReqBody>,
    ) -> Result<Response, Infallible>
    where
        S::Response: IntoResponse,
        S::Error: IntoResponse,
        S::Future: Send,
    {
        Ok(self.handle_request(inner, req).await)
    }

    /// Converts a [`SettleResponse`] into an HTTP header value.
    ///
    /// Returns an error response if conversion fails.
    fn settlement_to_header(
        &self,
        settlement: SettleResponse,
    ) -> Result<HeaderValue, Box<Response>> {
        let payment_header: Base64Bytes = settlement.try_into().map_err(|err| {
            X402Error::settlement_failed(err, self.payment_requirements.as_ref().clone())
                .into_response()
        })?;

        HeaderValue::from_bytes(payment_header.as_ref()).map_err(|err| {
            let response =
                X402Error::settlement_failed(err, self.payment_requirements.as_ref().clone())
                    .into_response();
            Box::new(response)
        })
    }

    /// Seal the response body to the payer and anchor it, returning the response
    /// (with its body intact) and the `X-Durable-Evidence` header value.
    ///
    /// The body has to be buffered to be encrypted, so it is read out and put
    /// back byte-for-byte. The buyer receives exactly what the handler produced;
    /// DX402 only observes it on the way past.
    ///
    /// Nothing in here can fail the request. Every error path yields a skip
    /// notice, and a response is always returned.
    /// Put the evidence object where the core specification reserves
    /// extension metadata: `SettlementResponse.extensions["durable-evidence"]`
    /// of the settlement the buyer receives in `X-Payment-Response`. The
    /// `X-Durable-Evidence` header keeps being emitted as a convenience.
    fn with_evidence(
        mut settlement: SettleResponse,
        value: Option<serde_json::Value>,
    ) -> SettleResponse {
        if let Some(value) = value {
            settlement
                .extensions
                .get_or_insert_with(Default::default)
                .insert(x402_rs::dx402::EXTENSION_KEY.to_string(), value);
        }
        settlement
    }

    async fn attach_durable_evidence(
        hook: &Arc<DurableEvidenceHook>,
        accepts: &[PaymentRequirements],
        extensions: &HashMap<String, serde_json::Value>,
        verify_request: &VerifyRequest,
        settlement: &SettleResponse,
        response: Response,
    ) -> (Response, Option<HeaderValue>, Option<serde_json::Value>) {
        use x402_rs::dx402::types::{DurableEvidence, SkipReason};

        // The buyer's choice comes first. If the route offered evidence and the
        // buyer paid for the offer without it, deliver and say so; nothing
        // below runs, so the plain offer costs the seller no sealing at all.
        let paid_index = accepts
            .iter()
            .position(|r| r == &verify_request.payment_requirements);
        let offer = match OfferDecision::decide(
            accepts,
            extensions,
            paid_index,
            &verify_request.payment_requirements,
        ) {
            OfferDecision::NotSelected => {
                let evidence = DurableEvidence::skipped(SkipReason::NotSelected);
                let header = encode_header(&evidence).and_then(|v| HeaderValue::from_str(&v).ok());
                let value = serde_json::to_value(&evidence).ok();
                return (response, header, value);
            }
            OfferDecision::Declared(cfg) => Some(cfg),
            OfferDecision::Legacy => None,
        };

        let Some(tx_hash) = settlement.transaction.as_ref().map(|t| t.to_string()) else {
            // Settled without a transaction hash: nothing stable to bind the
            // ciphertext to, so no evidence rather than evidence keyed on
            // something a verifier could not check.
            return (response, None, None);
        };

        let (parts, body) = response.into_parts();
        let limit = hook.config().max_body_bytes;

        // Claim the memory this capture may need BEFORE buffering a byte of it.
        // Refused rather than queued: the buffering happens ahead of the buyer's
        // response, so waiting here would delay a delivery that is already paid
        // for. Held until the seal and the upload are done, which is the whole
        // window where the plaintext and the ciphertext coexist.
        let permit = match hook.reserve_for(&body) {
            Ok(permit) => permit,
            Err(reason) => {
                let evidence = DurableEvidence::skipped(reason);
                let header = encode_header(&evidence).and_then(|v| HeaderValue::from_str(&v).ok());
                let value = serde_json::to_value(&evidence).ok();
                return (Response::from_parts(parts, body), header, value);
            }
        };

        // Skipping evidence must never change the bytes the buyer receives. The
        // settlement already happened (`settle_after_execution`), so returning an
        // empty body here would charge for goods and then deliver nothing --
        // unrecoverably, since the authorization nonce is spent.
        let bytes = match buffer_body(body, limit).await {
            BufferedBody::Ready(bytes) => bytes,
            BufferedBody::Skip { body, reason } => {
                hook.record_skip(reason);
                let evidence = DurableEvidence::skipped(reason);
                let header = encode_header(&evidence).and_then(|v| HeaderValue::from_str(&v).ok());
                let value = serde_json::to_value(&evidence).ok();
                return (Response::from_parts(parts, body), header, value);
            }
        };

        let payment_id = x402_rs::dx402::payment_id(settlement.network, &tx_hash);
        let payer_key = x402_rs::dx402::payer::payer_public_key(
            &verify_request.payment_payload,
            &verify_request.payment_requirements,
            &settlement.payer,
        );

        let ctx = SettledContext {
            offer,
            payment_id,
            network: settlement.network,
            tx_hash,
            payer: settlement.payer.clone(),
            payee: verify_request.payment_requirements.pay_to.clone(),
            proof: settlement.proof_of_payment.clone(),
        };

        let evidence = hook.capture(&bytes, payer_key, &ctx).await;
        drop(permit);

        let header = encode_header(&evidence).and_then(|v| HeaderValue::from_str(&v).ok());
        let value = serde_json::to_value(&evidence).ok();
        (
            Response::from_parts(parts, Body::from(bytes)),
            header,
            value,
        )
    }

    /// Calls the inner service with proper telemetry instrumentation.
    async fn call_inner<
        ReqBody,
        ResBody,
        S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>>,
    >(
        mut inner: S,
        req: http::Request<ReqBody>,
    ) -> Result<http::Response<ResBody>, S::Error>
    where
        S::Future: Send,
    {
        #[cfg(feature = "telemetry")]
        {
            inner
                .call(req)
                .instrument(tracing::info_span!("inner"))
                .await
        }
        #[cfg(not(feature = "telemetry"))]
        {
            inner.call(req).await
        }
    }

    /// Orchestrates the full payment lifecycle: verifies the request, calls to the inner handler, and settles the payment, returns proper HTTP response.
    #[cfg_attr(
        feature = "telemetry",
        instrument(name = "x402.handle_request", skip_all)
    )]
    pub async fn handle_request<
        ReqBody,
        ResBody,
        S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>>,
    >(
        self,
        inner: S,
        req: http::Request<ReqBody>,
    ) -> Response
    where
        S::Response: IntoResponse,
        S::Error: IntoResponse,
        S::Future: Send,
    {
        let payment_payload = match self.extract_payment_payload(req.headers()).await {
            Ok(payment_payload) => payment_payload,
            Err(err) => {
                #[cfg(feature = "telemetry")]
                tracing::event!(Level::INFO, status = "failed", "No valid payment provided");
                return err.into_response();
            }
        };
        let verify_request = match self.verify_payment(payment_payload).await {
            Ok(verify_request) => verify_request,
            Err(err) => return err.into_response(),
        };

        if self.settle_before_execution {
            // Settlement before execution: settle payment first, then call inner handler
            #[cfg(feature = "telemetry")]
            tracing::debug!("Settling payment before request execution");

            let settlement = match self.settle_payment(&verify_request).await {
                Ok(settlement) => settlement,
                Err(err) => return err.into_response(),
            };

            // Settlement succeeded, now execute the request
            let response = match Self::call_inner(inner, req).await {
                Ok(response) => response.into_response(),
                Err(err) => return err.into_response(),
            };

            // DX402 runs here too. This branch used to skip the hook entirely,
            // so a route with `with_durable_offer` + `settle_before_execution`
            // charged for evidence and emitted nothing -- not even a skip
            // (red team #9). The settlement is held until after the handler so
            // the delivered body and the settlement identity coexist, exactly
            // as in the other branch. Error responses are not evidence.
            let delivered =
                !(response.status().is_client_error() || response.status().is_server_error());
            let (response, durable_header, durable_value) = match &self.durable {
                Some(hook) if delivered => {
                    Self::attach_durable_evidence(
                        hook,
                        &self.payment_requirements,
                        &self.extensions,
                        &verify_request,
                        &settlement,
                        response,
                    )
                    .await
                }
                _ => (response, None, None),
            };
            let settlement = Self::with_evidence(settlement, durable_value);

            let header_value = match self.settlement_to_header(settlement) {
                Ok(header) => header,
                Err(response) => return *response,
            };

            let mut res = response;
            if let Some(value) = durable_header {
                res.headers_mut().insert(crate::durable::HEADER_NAME, value);
            }
            res.headers_mut().insert("X-Payment-Response", header_value);
            res.into_response()
        } else {
            // Settlement after execution (default): call inner handler first, then settle
            #[cfg(feature = "telemetry")]
            tracing::debug!("Settling payment after request execution");

            let response = match Self::call_inner(inner, req).await {
                Ok(response) => response,
                Err(err) => return err.into_response(),
            };

            if response.status().is_client_error() || response.status().is_server_error() {
                return response.into_response();
            }

            let settlement = match self.settle_payment(&verify_request).await {
                Ok(settlement) => settlement,
                Err(err) => return err.into_response(),
            };

            // DX402: seal and anchor the body before `settlement` is consumed by
            // the header encoder. This is the only point in the whole protocol
            // where the delivered bytes and the settlement identity coexist.
            let (response, durable_header, durable_value) = match &self.durable {
                Some(hook) => {
                    Self::attach_durable_evidence(
                        hook,
                        &self.payment_requirements,
                        &self.extensions,
                        &verify_request,
                        &settlement,
                        response.into_response(),
                    )
                    .await
                }
                None => (response.into_response(), None, None),
            };
            let settlement = Self::with_evidence(settlement, durable_value);

            let header_value = match self.settlement_to_header(settlement) {
                Ok(header) => header,
                Err(response) => return *response,
            };

            let mut res = response;
            if let Some(value) = durable_header {
                res.headers_mut().insert(crate::durable::HEADER_NAME, value);
            }
            res.headers_mut().insert("X-Payment-Response", header_value);
            res.into_response()
        }
    }
}

/// A variant of [`PaymentRequirements`] without the `resource` field.
/// This allows resources to be dynamically inferred per request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentRequirementsNoResource {
    pub scheme: Scheme,
    pub network: Network,
    pub max_amount_required: TokenAmount,
    // no resource: Url,
    pub description: String,
    pub mime_type: String,
    pub pay_to: MixedAddress,
    pub max_timeout_seconds: u64,
    pub asset: MixedAddress,
    pub extra: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
}

impl PaymentRequirementsNoResource {
    /// Converts this partial requirement into a full [`PaymentRequirements`]
    /// using the provided resource URL.
    pub fn to_payment_requirements(&self, resource: Url) -> PaymentRequirements {
        PaymentRequirements {
            scheme: self.scheme,
            network: self.network,
            max_amount_required: self.max_amount_required,
            resource,
            description: self.description.clone(),
            mime_type: self.mime_type.clone(),
            pay_to: self.pay_to.clone(),
            max_timeout_seconds: self.max_timeout_seconds,
            asset: self.asset.clone(),
            extra: self.extra.clone(),
            output_schema: self.output_schema.clone(),
        }
    }
}

/// The offer a payload pays for, out of everything the route accepts.
///
/// Scheme and network narrow the field; when more than one offer survives --
/// the normal shape once a route lists the same resource with and without
/// `durable-evidence` -- the payload has to pick between them, and for an EVM
/// authorization it can: `to` and `value` are signed, so they are what the
/// buyer committed to. Taking the first survivor instead meant "what was paid"
/// was decided by the seller's listing order, not by the buyer: durable listed
/// first made the plain offer fail verification on amount, plain listed first
/// anchored `not_selected` for a buyer who had paid the higher price.
///
/// A lone survivor is returned as before, so single-offer routes -- every
/// integrator before offers existed -- see no change. Non-EVM payloads do not
/// expose the paid amount without parsing a transaction, so they keep the
/// first survivor; a route offering two same-network non-EVM entries is
/// ambiguous there, and a seller doing that today gets what it always got.
fn match_paid_requirement(
    accepts: &[PaymentRequirements],
    payment_payload: &PaymentPayload,
) -> Option<PaymentRequirements> {
    let survivors: Vec<&PaymentRequirements> = accepts
        .iter()
        .filter(|r| r.scheme == payment_payload.scheme && r.network == payment_payload.network)
        .collect();
    if survivors.len() <= 1 {
        return survivors.first().map(|r| (*r).clone());
    }
    if let ExactPaymentPayload::Evm(evm) = &payment_payload.payload {
        let paid_to: MixedAddress = evm.authorization.to.into();
        let value = evm.authorization.value;
        // Deref-coerced at each call site: the iterator hands out `&&T` and
        // `max_by_key` adds one more `&`.
        fn declares(r: &PaymentRequirements) -> bool {
            DurableEvidenceConfig::declared_on(r)
        }
        // Exact match first. Among several -- the seller priced the durable
        // offer the same as the plain one, or padded a plain tag to the durable
        // price -- the DECLARING offer wins: it is the only one a buyer can
        // have paid that amount for on purpose. Taking the first let a seller
        // bill the durable price and answer `not_selected` (red team #13).
        if let Some(exact) = survivors
            .iter()
            .filter(|r| r.pay_to == paid_to && r.max_amount_required == value)
            .max_by_key(|r| declares(r))
        {
            return Some((*exact).clone());
        }
        // No exact match: the buyer authorised more than any offer asks
        // (`maxAmountRequired` reads as a ceiling to some clients). The most
        // expensive offer the payment covers is what they bought; verify
        // accepts `>=` on EVM, so this is the offer that will actually settle.
        if let Some(covered) = survivors
            .iter()
            .filter(|r| r.pay_to == paid_to && r.max_amount_required <= value)
            .max_by_key(|r| (r.max_amount_required, declares(r)))
        {
            return Some((*covered).clone());
        }
    }
    survivors.first().map(|r| (*r).clone())
}

impl<F> X402Middleware<F> {
    /// Every offer on the route: the plain price tags first, then the ones
    /// that carry `durable-evidence`. Plain first on purpose -- a client that
    /// knows nothing of the extension takes the first acceptable entry, and
    /// that has to be the one without it.
    fn all_offers(&self) -> impl Iterator<Item = (&PriceTag, Option<&DurableEvidenceConfig>)> {
        self.price_tag
            .iter()
            .map(|t| (t, None))
            .chain(self.durable_offers.iter().map(|(t, c)| (t, Some(c))))
    }
}

/// The `extra` for one offer: the token's EIP-712 domain, plus the
/// `durable-evidence` declaration when this offer carries one.
fn extra_for(
    price_tag: &PriceTag,
    durable: Option<&DurableEvidenceConfig>,
) -> Option<serde_json::Value> {
    let mut extra = price_tag.token.eip712.clone().map(|eip712| {
        json!({
            "name": eip712.name,
            "version": eip712.version
        })
    });
    if let Some(cfg) = durable {
        // Declared through the same helper the hook reads with, so the two
        // sides cannot drift on where the declaration lives.
        let mut carrier = PaymentRequirements {
            scheme: Scheme::Exact,
            network: price_tag.token.network(),
            max_amount_required: price_tag.amount,
            resource: Url::parse("https://declare.invalid/").expect("static url"),
            description: String::new(),
            mime_type: String::new(),
            pay_to: price_tag.pay_to.clone(),
            max_timeout_seconds: 0,
            asset: price_tag.token.address(),
            extra: extra.take(),
            output_schema: None,
        };
        cfg.declare_on(&mut carrier);
        extra = carrier.extra;
    }
    extra
}

/// Enum capturing either fully constructed [`PaymentRequirements`] (with `resource`)
/// or resource-less variants that must be completed at runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaymentOffers {
    /// [`PaymentRequirements`] with static `resource` field.
    Ready(Arc<Vec<PaymentRequirements>>),
    /// [`PaymentRequirements`] lacking `resource`, to be added per request.
    NoResource {
        partial: Vec<PaymentRequirementsNoResource>,
        base_url: Url,
    },
    /// Dynamically computed price per request using a user-provided callback.
    /// The resource URL is automatically constructed from base URL and request URI.
    DynamicPrice {
        partial: Vec<PaymentRequirementsNoResource>,
        base_url: Url,
        price_callback: DynamicPriceFn,
    },
}

/// Constructs a full list of [`PaymentRequirements`] for a request.
///
/// This function returns a shared, reference-counted vector of [`PaymentRequirements`]
/// based on the provided [`PaymentOffers`].
///
/// - If `payment_offers` is [`PaymentOffers::Ready`], it returns an Arc clone of the precomputed requirements.
/// - If `payment_offers` is [`PaymentOffers::NoResource`], it dynamically constructs the `resource` URI
///   by combining the `base_url` with the request's path and query, and completes each
///   partial `PaymentRequirementsNoResource` into a full `PaymentRequirements`.
/// - If `payment_offers` is [`PaymentOffers::DynamicPrice`], it constructs the resource URL,
///   calls the price callback to get the dynamic price, updates all partial requirements with
///   the new price, and converts them to full `PaymentRequirements`.
///
/// # Arguments
///
/// * `payment_offers` - The current payment offer configuration, either precomputed or partial.
/// * `req_uri` - The incoming request URI used to construct the full resource path if needed.
/// * `req_headers` - The incoming request headers passed to the price callback if needed.
///
/// # Returns
///
/// An `Arc<Vec<PaymentRequirements>>` ready to be passed to a facilitator for verification.
async fn gather_payment_requirements(
    payment_offers: &PaymentOffers,
    req_uri: &Uri,
    req_headers: &HeaderMap,
) -> Arc<Vec<PaymentRequirements>> {
    match payment_offers {
        PaymentOffers::Ready(requirements) => {
            // requirements is &Arc<Vec<PaymentRequirements>>
            Arc::clone(requirements)
        }
        PaymentOffers::NoResource { partial, base_url } => {
            let resource = {
                let mut resource_url = base_url.clone();
                resource_url.set_path(req_uri.path());
                resource_url.set_query(req_uri.query());
                resource_url
            };
            let payment_requirements = partial
                .iter()
                .map(|partial| partial.to_payment_requirements(resource.clone()))
                .collect::<Vec<_>>();
            Arc::new(payment_requirements)
        }
        PaymentOffers::DynamicPrice {
            partial,
            base_url,
            price_callback,
        } => {
            // Build resource URL from base_url and request URI
            let resource = {
                let mut resource_url = base_url.clone();
                resource_url.set_path(req_uri.path());
                resource_url.set_query(req_uri.query());
                resource_url
            };

            // Call the price callback to get the dynamic price
            match price_callback
                .get_price(req_headers, req_uri, base_url)
                .await
            {
                Ok(dynamic_price) => {
                    // Update all partial requirements with the dynamic price
                    let payment_requirements = partial
                        .iter()
                        .map(|partial| {
                            let mut req = partial.to_payment_requirements(resource.clone());
                            req.max_amount_required = dynamic_price;
                            req
                        })
                        .collect::<Vec<_>>();
                    Arc::new(payment_requirements)
                }
                Err(_) => {
                    // If price callback fails, fall back to NoResource behavior (use original prices)
                    let payment_requirements = partial
                        .iter()
                        .map(|partial| partial.to_payment_requirements(resource.clone()))
                        .collect::<Vec<_>>();
                    Arc::new(payment_requirements)
                }
            }
        }
    }
}

/// Type alias for a dynamic price callback function signature.
///
/// This callback receives request headers, URI, and base URL, and returns
/// the price amount for the request. It's used with [`X402Middleware::with_dynamic_price`].
///
/// The callback signature is:
/// ```rust,ignore
/// for<'a> Fn(
///     &'a HeaderMap,
///     &'a Uri,
///     &'a Url,
/// ) -> Pin<Box<dyn Future<Output = Result<TokenAmount, X402Error>> + Send + 'a>>
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use x402_axum::layer::DynamicPriceCallback;
/// use x402_rs::types::TokenAmount;
///
/// // Define your price calculation logic
/// async fn calculate_price(
///     headers: &HeaderMap,
///     uri: &Uri,
///     base_url: &Url,
/// ) -> Result<TokenAmount, X402Error> {
///     // Extract price from headers, cache, or compute dynamically
///     Ok(TokenAmount::from(1000000))
/// }
///
/// // Use it with with_dynamic_price
/// let callback = move |headers: &HeaderMap, uri: &Uri, base_url: &Url| {
///     Box::pin(calculate_price(headers, uri, base_url))
/// };
///
/// x402.with_dynamic_price(callback);
/// ```
pub type DynamicPriceCallback = dyn for<'a> Fn(
        &'a HeaderMap,
        &'a Uri,
        &'a Url,
    ) -> Pin<Box<dyn Future<Output = Result<TokenAmount, X402Error>> + Send + 'a>>
    + Send
    + Sync;

/// A clonable wrapper for an async price callback function that computes per-request prices.
#[derive(Clone)]
pub struct DynamicPriceFn(Arc<DynamicPriceCallback>);

impl Debug for DynamicPriceFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DynamicPriceFn(<function>)")
    }
}

impl PartialEq for DynamicPriceFn {
    fn eq(&self, _other: &Self) -> bool {
        // Function pointers can't be meaningfully compared for equality
        false
    }
}

impl Eq for DynamicPriceFn {}

impl DynamicPriceFn {
    pub fn new<P>(price_callback: P) -> Self
    where
        P: for<'a> Fn(
                &'a HeaderMap,
                &'a Uri,
                &'a Url,
            )
                -> Pin<Box<dyn Future<Output = Result<TokenAmount, X402Error>> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        DynamicPriceFn(Arc::new(price_callback))
    }

    pub async fn get_price(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        base_url: &Url,
    ) -> Result<TokenAmount, X402Error> {
        (self.0)(headers, uri, base_url).await
    }
}

#[cfg(test)]
mod paid_offer_tests {
    use super::*;
    use x402_rs::dx402::DurableEvidenceConfig;

    fn offer(amount: &str) -> PaymentRequirements {
        serde_json::from_value(json!({
            "scheme": "exact", "network": "base", "maxAmountRequired": amount,
            "resource": "https://kk.example/data/42", "description": "d",
            "mimeType": "application/json",
            "payTo": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
            "maxTimeoutSeconds": 300,
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        }))
        .unwrap()
    }

    fn paid(value: &str) -> PaymentPayload {
        serde_json::from_value(json!({
            "x402Version": 1, "scheme": "exact", "network": "base",
            "payload": {
                "signature": format!("0x{}", "11".repeat(65)),
                "authorization": {
                    "from": "0x103040545AC5031A11E8C03dd11324C7333a13C7",
                    "to": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
                    "value": value,
                    "validAfter": "0", "validBefore": "9999999999",
                    "nonce": format!("0x{}", "22".repeat(32))
                }
            }
        }))
        .unwrap()
    }

    fn pair(durable_first: bool) -> Vec<PaymentRequirements> {
        let plain = offer("10000");
        let mut durable = offer("12000");
        DurableEvidenceConfig::default().declare_on(&mut durable);
        if durable_first {
            vec![durable, plain]
        } else {
            vec![plain, durable]
        }
    }

    #[test]
    fn the_buyer_decides_which_offer_was_paid_not_the_listing_order() {
        // Found by the SDK-parity review: with scheme+network matching and
        // "take the first", listing order decided what the buyer had bought.
        for durable_first in [false, true] {
            let accepts = pair(durable_first);
            let chosen = match_paid_requirement(&accepts, &paid("12000")).unwrap();
            assert!(
                DurableEvidenceConfig::from_requirements(&chosen).is_some(),
                "paid 12000 -> the durable offer, listed {}",
                if durable_first { "first" } else { "second" }
            );
            let chosen = match_paid_requirement(&accepts, &paid("10000")).unwrap();
            assert!(
                DurableEvidenceConfig::from_requirements(&chosen).is_none(),
                "paid 10000 -> the plain offer, listed {}",
                if durable_first { "second" } else { "first" }
            );
        }
    }

    #[test]
    fn an_equal_price_tie_goes_to_the_offer_that_declares() {
        // "Evidence at no extra charge" is the first thing a seller will try.
        // With first-match it was dead: every buyer got `not_selected`. And
        // the same rule closes the skim -- a plain tag padded to the durable
        // price no longer captures a buyer who paid for evidence.
        let plain = offer("12000");
        let mut durable = offer("12000");
        DurableEvidenceConfig::default().declare_on(&mut durable);
        for accepts in [vec![plain.clone(), durable.clone()], vec![durable, plain]] {
            let chosen = match_paid_requirement(&accepts, &paid("12000")).unwrap();
            assert!(DurableEvidenceConfig::from_requirements(&chosen).is_some());
        }
    }

    #[test]
    fn a_buyer_that_overpays_gets_the_most_expensive_offer_it_covered() {
        // Some clients read `maxAmountRequired` as a ceiling and authorise
        // more. That used to fall to "first survivor" = plain: overpaid, no
        // evidence, no error.
        let accepts = pair(false);
        let chosen = match_paid_requirement(&accepts, &paid("15000")).unwrap();
        assert_eq!(chosen.max_amount_required.to_string(), "12000");
        assert!(DurableEvidenceConfig::from_requirements(&chosen).is_some());
    }

    #[test]
    fn a_single_offer_route_is_untouched() {
        // Every integrator from before offers existed. The value may not even
        // match on a loose client; the facilitator's verify decides that, as
        // it always did.
        let only = vec![offer("10000")];
        let chosen = match_paid_requirement(&only, &paid("99999")).unwrap();
        assert_eq!(chosen.max_amount_required.to_string(), "10000");
    }

    #[test]
    fn a_value_matching_no_offer_falls_back_rather_than_refusing_here() {
        // Refusal belongs to verify, which reports the amount mismatch with the
        // real requirements attached. Matching only has to be deterministic.
        let accepts = pair(false);
        assert!(match_paid_requirement(&accepts, &paid("5")).is_some());
    }
}
