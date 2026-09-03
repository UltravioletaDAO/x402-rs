//! Middleware for handling HTTP 402 Payment Required responses using the x402 protocol.
//!
//! This module provides the `X402Payments` struct which implements `reqwest_middleware::Middleware`,
//! allowing automatic retries of requests with valid `X-Payment` headers constructed via a signer.
//!
//! It includes:
//! - Selection of preferred payment methods
//! - Max token enforcement
//! - EIP-712-based payload construction and signing
//! - Base64 encoding into a payment header

use http::{Extensions, HeaderValue, StatusCode};
use reqwest::{Request, Response};
use reqwest_middleware as rqm;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTimeError;
use tracing::instrument;
use x402_rs::dx402::DurableEvidenceConfig;
use x402_rs::network::{Network, USDCDeployment};
use x402_rs::types::{
    Base64Bytes, MixedAddressError, MoneyAmount, MoneyAmountParseError, PaymentPayload,
    PaymentRequiredResponse, PaymentRequirements, TokenAmount, TokenAsset, TokenDeployment,
};

use crate::chains::{IntoSenderWallet, SenderWallet};

/// Represents the maximum allowed amount for a specific token asset.
pub struct MaxTokenAmount {
    asset: TokenAsset,
    amount: TokenAmount,
}

/// Trait for converting from a token amount directly into a MaxTokenAmount bound.
pub trait MaxTokenAmountFromTokenAmount {
    fn token_amount<A: Into<TokenAmount>>(&self, token_amount: A) -> MaxTokenAmount;
}

/// Trait for converting from a user-friendly amount (e.g., "1.0 USDC")
/// into a token-denominated max bound, respecting decimals.
pub trait MaxTokenAmountFromAmount {
    type Error;
    fn amount<A: TryInto<MoneyAmount>>(&self, amount: A) -> Result<MaxTokenAmount, Self::Error>;
}

impl MaxTokenAmountFromTokenAmount for TokenAsset {
    fn token_amount<A: Into<TokenAmount>>(&self, token_amount: A) -> MaxTokenAmount {
        MaxTokenAmount {
            asset: self.clone(),
            amount: token_amount.into(),
        }
    }
}

impl MaxTokenAmountFromTokenAmount for TokenDeployment {
    fn token_amount<A: Into<TokenAmount>>(&self, token_amount: A) -> MaxTokenAmount {
        MaxTokenAmount {
            asset: self.asset.clone(),
            amount: token_amount.into(),
        }
    }
}

/// Errors that can occur while constructing or applying an x402 payment.
#[derive(Debug, thiserror::Error)]
pub enum X402PaymentsError {
    /// Occurs when a value fails to convert into a [`MoneyAmount`],
    /// for example, parsing a string like `"1.0"` fails due to formatting or type mismatch.
    #[error("Failed to convert to MoneyAmount")]
    MoneyAmountConversion,
    /// Occurs when a [`MoneyAmount`] cannot be converted into a [`TokenAmount`],
    /// typically due to a decimal mismatch or overflow,
    /// for example, trying to convert `0.00000000001` to a USDC token amount (which has 6 decimals).
    #[error("Failed to convert to TokenAmount")]
    TokenAmountConversion(#[source] MoneyAmountParseError),
    /// Triggered when the selected payment amount exceeds the configured maximum for that token.
    /// This prevents accidental or malicious overspending.
    #[error("Payment amount {requested} exceeds maximum allowed {allowed} for token {asset}")]
    PaymentAmountTooLarge {
        requested: TokenAmount,
        allowed: TokenAmount,
        asset: TokenAsset,
    },
    /// Indicates that the original request could not be cloned for retrying with a payment header.
    /// This typically happens when the request body is a stream or otherwise non-reusable.
    #[error("Request object is not cloneable. Are you passing a streaming body?")]
    RequestNotCloneable,
    /// Raised when none of the server's accepted payment methods match the client's preferred tokens.
    /// Includes both the accepted and preferred sets to aid debugging.
    #[error("No matching payment method found. Accepted: {accepts:?}. Preferred: {prefer:?}")]
    NoSuitablePaymentMethod {
        accepts: Vec<PaymentRequirements>,
        prefer: Vec<TokenAsset>,
    },
    /// Raised when an EVM address (e.g., `to`, `from`, or `verifying_contract`) is invalid or cannot be parsed.
    #[error("Invalid EVM address")]
    InvalidEVMAddress(#[source] MixedAddressError),
    /// Raised when the system clock could not be read to compute `validAfter`/`validBefore` timestamps.
    /// Should be an extremely rare occurrence.
    #[error("Failed to get system clock")]
    ClockError(#[source] SystemTimeError),
    /// Indicates that signing the EIP-712 payment payload failed using the provided signer.
    #[error("Failed to sign payment payload: {0}")]
    SigningError(String),
    /// Occurs if the constructed payment payload cannot be serialized to JSON.
    /// This should be an extremely rare occurrence.
    #[error("Failed to encode payment payload to json")]
    JsonEncodeError(#[source] serde_json::Error),
    /// Raised when the base64-encoded JSON payload cannot be inserted into a [`HeaderValue`].
    /// Typically caused by invalid characters or excessive length.
    #[error("Failed to encode payment payload to HTTP header")]
    HeaderValueEncodeError(#[source] http::header::InvalidHeaderValue),
}

impl From<X402PaymentsError> for rqm::Error {
    fn from(error: X402PaymentsError) -> Self {
        rqm::Error::Middleware(error.into())
    }
}

impl MaxTokenAmountFromAmount for TokenDeployment {
    type Error = X402PaymentsError;
    fn amount<A: TryInto<MoneyAmount>>(&self, amount: A) -> Result<MaxTokenAmount, Self::Error> {
        let money_amount = amount
            .try_into()
            .map_err(|_| Self::Error::MoneyAmountConversion)?;
        let decimals = self.decimals;
        let token_amount = money_amount
            .as_token_amount(decimals as u32)
            .map_err(Self::Error::TokenAmountConversion)?;
        Ok(MaxTokenAmount {
            asset: self.asset.clone(),
            amount: token_amount,
        })
    }
}

/// Middleware that handles automatic retries for HTTP 402 responses
/// by attaching a valid x402 payment header.
#[derive(Clone)]
pub struct X402Payments {
    wallets: Vec<Arc<dyn SenderWallet>>,
    max_token_amount: HashMap<TokenAsset, TokenAmount>,
    prefer: Vec<TokenAsset>,
    /// When a seller offers the same resource with and without
    /// `durable-evidence`, take the one with it.
    ///
    /// Off by default on purpose: the offer with evidence usually costs more,
    /// and a client that never asked for durability must not start paying for
    /// it because a seller began offering it.
    prefer_durable_evidence: bool,
}

impl X402Payments {
    pub fn with_wallet<S: IntoSenderWallet>(wallet: S) -> Self {
        Self {
            wallets: vec![wallet.into_sender_wallet()],
            max_token_amount: HashMap::new(),
            prefer: vec![],
            prefer_durable_evidence: false,
        }
    }

    pub fn and_with_wallet<S: IntoSenderWallet>(self, wallet: S) -> Self {
        let mut wallets = self.wallets;
        wallets.push(wallet.into_sender_wallet());
        Self {
            wallets,
            max_token_amount: self.max_token_amount,
            prefer: self.prefer,
            prefer_durable_evidence: self.prefer_durable_evidence,
        }
    }

    /// Set a max amount allowed for a given token.
    pub fn max(&self, max: MaxTokenAmount) -> Self {
        let mut this = self.clone();
        this.max_token_amount.insert(max.asset, max.amount);
        this
    }

    /// Extend the preferred token list, prioritizing what the client wants to pay with.
    pub fn prefer<T: Into<Vec<TokenAsset>>>(&self, prefer: T) -> Self {
        let mut this = self.clone();
        this.prefer.append(&mut prefer.into());
        this
    }

    /// Prefer the offer that carries `durable-evidence` when a seller offers both.
    ///
    /// This is the buyer's opt-in to DX402. It rides on the multi-offer
    /// `accepts` array x402 already has, so nothing in the protocol changes:
    /// the seller lists the resource twice, this picks the one with evidence,
    /// and the seller sees which one was paid.
    pub fn prefer_durable_evidence(&self) -> Self {
        let mut this = self.clone();
        this.prefer_durable_evidence = true;
        this
    }

    /// Selects the most preferred payment requirement based on the client's `prefer` list
    /// and network priority (Base preferred).
    pub fn select_payment_requirements(
        &self,
        payment_requirements: &[PaymentRequirements],
    ) -> Result<PaymentRequirements, X402PaymentsError> {
        let mut sorted: Vec<PaymentRequirements> = payment_requirements.to_vec();
        // Assign priority score: lower is better
        // Prefer what is in self.prefer and ultimately Base
        sorted.sort_by_key(|req| {
            let pref_index = self
                .prefer
                .iter()
                .position(|a| a == &req.token_asset())
                .unwrap_or(usize::MAX);
            let base_priority = if req.network == Network::Base { 0 } else { 1 };
            // Among otherwise-equal offers, the one with evidence goes LAST
            // unless the client asked for it. Deterministic regardless of the
            // order the seller listed them in, so a seller cannot make an
            // indifferent client pay for evidence by listing it first.
            let durable = DurableEvidenceConfig::from_requirements(req).is_some();
            let durable_rank = if self.prefer_durable_evidence {
                u8::from(!durable)
            } else {
                u8::from(durable)
            };
            (pref_index, base_priority, durable_rank)
        });

        #[cfg(feature = "telemetry")]
        {
            for (i, req) in sorted.iter().enumerate() {
                tracing::debug!(index = i, asset = ?req.asset, network = ?req.network, "Ranked candidate payment requirement");
            }
        }

        // Try to find a USDC requirement (networks without a known USDC deployment
        // return None from by_network and simply do not match).
        let usdc_requirement = sorted.iter().find(|req| {
            USDCDeployment::by_network(req.network)
                .map(|usdc| req.asset == usdc.address())
                .unwrap_or(false)
        });

        let selected = usdc_requirement
            .cloned() // Prioritize USDC requirements if available
            .or_else(|| sorted.into_iter().next()); // If no USDC requirements are found, return the first accepted requirement.

        selected.ok_or(X402PaymentsError::NoSuitablePaymentMethod {
            accepts: payment_requirements.to_vec(),
            prefer: self.prefer.clone(),
        })
    }

    /// Ensures that the selected requirement does not exceed the max configured amount.
    pub fn assert_max_amount(
        &self,
        selected: &PaymentRequirements,
    ) -> Result<(), X402PaymentsError> {
        let token_asset = selected.token_asset();
        if let Some(max) = self.max_token_amount.get(&token_asset) {
            if &selected.max_amount_required > max {
                return Err(X402PaymentsError::PaymentAmountTooLarge {
                    requested: selected.max_amount_required,
                    allowed: *max,
                    asset: token_asset,
                });
            }
        }
        Ok(())
    }

    /// Constructs a [`PaymentPayload`] for a given requirement by generating
    /// a nonce and signing an EIP-712 [`TransferWithAuthorization`] struct.
    #[instrument(name = "x402.make_payment_payload", skip_all, fields(
        network = ?selected.network,
        token = ?selected.asset,
        amount = %selected.max_amount_required,
    ))]
    pub async fn make_payment_payload(
        &self,
        selected: PaymentRequirements,
    ) -> Result<PaymentPayload, X402PaymentsError> {
        let wallet = self.wallets.iter().find(|w| w.can_handle(&selected));
        match wallet {
            None => Err(X402PaymentsError::SigningError(
                "No suitable wallet found".to_string(),
            )),
            Some(wallet) => wallet.payment_payload(selected).await,
        }
    }

    /// Encodes the `PaymentPayload` into a base64 string suitable for an `X-Payment` header.
    pub fn encode_payment_header(
        payload: &PaymentPayload,
    ) -> Result<HeaderValue, X402PaymentsError> {
        let json = serde_json::to_vec(payload).map_err(X402PaymentsError::JsonEncodeError)?;
        let b64 = Base64Bytes::encode(json);
        HeaderValue::from_bytes(b64.as_ref()).map_err(X402PaymentsError::HeaderValueEncodeError)
    }

    /// Builds the payment header by selecting a requirement, enforcing max,
    /// constructing and signing the payload, and base64-encoding it.
    #[instrument(name = "x402.build_payment_header", skip(self))]
    pub async fn build_payment_header(
        &self,
        accepts: &[PaymentRequirements],
    ) -> Result<HeaderValue, X402PaymentsError> {
        let selected = self.select_payment_requirements(accepts)?;
        #[cfg(feature = "telemetry")]
        tracing::debug!(?selected, "Selected payment requirement");
        self.assert_max_amount(&selected)?;
        let payment_payload = self.make_payment_payload(selected).await?;
        Self::encode_payment_header(&payment_payload)
    }
}

#[async_trait::async_trait]
impl rqm::Middleware for X402Payments {
    /// Intercepts the response. If it's a 402, it constructs a payment and retries the request.
    #[instrument(name = "x402.handle", skip(self, req, extensions, next), fields(method = %req.method(), url = %req.url()))]
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: rqm::Next<'_>,
    ) -> rqm::Result<Response> {
        let retry_req = req.try_clone(); // For retrying with payment later

        let res = next.clone().run(req, extensions).await?;

        #[cfg(feature = "telemetry")]
        tracing::debug!("Received response: {}", res.status());

        if res.status() != StatusCode::PAYMENT_REQUIRED {
            return Ok(res); // No 402 needed: passthrough
        }

        #[cfg(feature = "telemetry")]
        tracing::debug!("Received 402 Payment Required");

        let payment_required_response = challenge_from(res).await?;

        let retry_req = async {
            let payment_header = self
                .build_payment_header(&payment_required_response.accepts)
                .await?;
            let mut req = retry_req.ok_or(X402PaymentsError::RequestNotCloneable)?;
            let headers = req.headers_mut();
            headers.insert("X-Payment", payment_header);
            headers.insert(
                "Access-Control-Expose-Headers",
                HeaderValue::from_static("X-Payment-Response"),
            );
            Ok::<Request, X402PaymentsError>(req)
        }
        .await
        .map_err(Into::<rqm::Error>::into)?;
        next.run(retry_req, extensions).await
    }
}

/// Read the 402 challenge from wherever the seller put it.
///
/// x402 allows BOTH transports and sellers pick freely:
///
/// * base64 JSON in the `PAYMENT-REQUIRED` (or `X-PAYMENT-REQUIRED`) header
/// * JSON in the response body
///
/// This read the body only. Measured against production on 2026-08-20, that was
/// the wrong half: of 40 live Bazaar resources, **36 of 36 that answered 402
/// carried the challenge in the header and none in the body**. Worse, sellers
/// like Tenjin use the body for a free preview of the paid content -- so the
/// body is valid JSON that simply has no `accepts`, and this failed to
/// deserialize rather than finding the terms one header away.
///
/// The header is tried first because that is where live sellers put it; the body
/// is the fallback so the sellers who use it keep working.
async fn challenge_from(res: Response) -> Result<PaymentRequiredResponse, reqwest::Error> {
    use base64::Engine as _;

    let header = res
        .headers()
        .get("payment-required")
        .or_else(|| res.headers().get("x-payment-required"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    if let Some(raw) = header {
        let raw = raw.trim();
        // Bare JSON first (a few sellers skip the encoding), then base64.
        let decoded = serde_json::from_str::<PaymentRequiredResponse>(raw)
            .ok()
            .or_else(|| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(raw)
                    .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw))
                    .ok()?;
                serde_json::from_slice::<PaymentRequiredResponse>(&bytes).ok()
            });
        if let Some(challenge) = decoded {
            #[cfg(feature = "telemetry")]
            tracing::debug!("402 challenge read from the PAYMENT-REQUIRED header");
            return Ok(challenge);
        }
        // An unparseable header is not fatal on its own -- fall through to the
        // body rather than refusing a seller whose header we simply did not
        // understand.
        #[cfg(feature = "telemetry")]
        tracing::debug!("PAYMENT-REQUIRED header present but unparseable; trying the body");
    }

    res.json::<PaymentRequiredResponse>().await
}

#[cfg(test)]
mod challenge_transport_tests {
    use super::*;

    /// A real `PAYMENT-REQUIRED` header value: base64 of an x402 challenge, the
    /// shape live sellers serve.
    const HEADER_CHALLENGE: &str = "eyJ4NDAyVmVyc2lvbiI6IDEsICJlcnJvciI6ICJQYXltZW50IHJlcXVpcmVkIiwgImFjY2VwdHMiOiBbeyJzY2hlbWUiOiAiZXhhY3QiLCAibmV0d29yayI6ICJiYXNlIiwgIm1heEFtb3VudFJlcXVpcmVkIjogIjEwMDAwMCIsICJyZXNvdXJjZSI6ICJodHRwczovL3Rlbmppbi5ibG9nL3giLCAiZGVzY3JpcHRpb24iOiAiZCIsICJtaW1lVHlwZSI6ICJhcHBsaWNhdGlvbi9qc29uIiwgInBheVRvIjogIjB4YjA1OWVBQzkzMzBEQzVmMjNGNTM0NmE4MTM0OEFmMUU5OWYzNzliZCIsICJtYXhUaW1lb3V0U2Vjb25kcyI6IDMwMCwgImFzc2V0IjogIjB4ODMzNTg5ZkNENmVEYjZFMDhmNGM3QzMyRDRmNzFiNTRiZEEwMjkxMyJ9XX0=";

    /// What a real seller puts in the 402 BODY: a free preview of the paid
    /// content. Valid JSON, no `accepts` -- so deserializing it as a challenge
    /// fails, which is exactly how this crate could not pay them.
    const PREVIEW_BODY: &str = r#"{"id":"01a01a4c","slug":"china-macro-weekly","title":"China Macro Weekly","price":"100000"}"#;

    fn response(header: Option<&str>, body: &str) -> Response {
        let mut builder = http::Response::builder().status(402);
        if let Some(h) = header {
            builder = builder.header("payment-required", h);
        }
        Response::from(builder.body(body.to_string()).unwrap())
    }

    #[tokio::test]
    async fn the_challenge_is_found_in_the_header() {
        // Measured 2026-08-20: 36 of 36 live Bazaar resources answering 402 put
        // the challenge here and none in the body. Reading the body only meant
        // this crate could not pay any of them.
        let res = response(Some(HEADER_CHALLENGE), PREVIEW_BODY);
        let challenge = challenge_from(res).await.expect("header transport");
        assert_eq!(challenge.accepts.len(), 1);
        assert_eq!(
            challenge.accepts[0].pay_to.to_string().to_lowercase(),
            "0xb059eac9330dc5f23f5346a81348af1e99f379bd"
        );
    }

    #[tokio::test]
    async fn the_body_transport_still_works() {
        // Both are legal. Supporting the header must not drop the sellers who
        // use the body.
        let body = r#"{"x402Version":1,"error":"Payment required","accepts":[{"scheme":"exact","network":"base","maxAmountRequired":"1","resource":"https://x.test/","description":"d","mimeType":"application/json","payTo":"0xb059eAC9330DC5f23F5346a81348Af1E99f379bd","maxTimeoutSeconds":300,"asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"}]}"#;
        let challenge = challenge_from(response(None, body))
            .await
            .expect("body transport");
        assert_eq!(challenge.accepts.len(), 1);
    }

    #[tokio::test]
    async fn an_unparseable_header_falls_through_to_the_body() {
        // A header we do not understand must not refuse a seller whose body is
        // perfectly good.
        let body = r#"{"x402Version":1,"error":"Payment required","accepts":[{"scheme":"exact","network":"base","maxAmountRequired":"1","resource":"https://x.test/","description":"d","mimeType":"application/json","payTo":"0xb059eAC9330DC5f23F5346a81348Af1E99f379bd","maxTimeoutSeconds":300,"asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"}]}"#;
        let challenge = challenge_from(response(Some("!!!not base64!!!"), body))
            .await
            .expect("must fall back rather than refuse");
        assert_eq!(challenge.accepts.len(), 1);
    }

    #[tokio::test]
    async fn neither_transport_is_still_an_error() {
        // No challenge anywhere is a genuine failure, and must stay loud.
        assert!(challenge_from(response(None, PREVIEW_BODY)).await.is_err());
    }
}

#[cfg(test)]
mod durable_offer_tests {
    use super::*;
    use x402_rs::dx402::Retention;

    fn offer(amount: &str) -> PaymentRequirements {
        serde_json::from_value(serde_json::json!({
            "scheme": "exact", "network": "base", "maxAmountRequired": amount,
            "resource": "https://kk.example/data/42", "description": "d",
            "mimeType": "application/json",
            "payTo": "0xb059eAC9330DC5f23F5346a81348Af1E99f379bd",
            "maxTimeoutSeconds": 300,
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        }))
        .unwrap()
    }

    fn pair() -> Vec<PaymentRequirements> {
        let plain = offer("10000");
        let mut durable = offer("12000");
        DurableEvidenceConfig {
            retention: Retention::Year1,
            ..Default::default()
        }
        .declare_on(&mut durable);
        vec![plain, durable]
    }

    fn client() -> X402Payments {
        // No wallet needed to rank offers; a wallet is only used to sign.
        X402Payments {
            wallets: vec![],
            max_token_amount: HashMap::new(),
            prefer: vec![],
            prefer_durable_evidence: false,
        }
    }

    #[test]
    fn an_indifferent_client_pays_for_the_plain_offer() {
        // Degradation is the property that makes this proposable: a client
        // that never heard of the extension keeps paying what it paid.
        let chosen = client().select_payment_requirements(&pair()).unwrap();
        assert_eq!(chosen.max_amount_required.to_string(), "10000");
        assert!(DurableEvidenceConfig::from_requirements(&chosen).is_none());
    }

    #[test]
    fn the_order_the_seller_lists_them_in_does_not_matter() {
        // Listing the dearer offer first must not make an indifferent client
        // pay for evidence. The tie-break is explicit, not positional.
        let mut reversed = pair();
        reversed.reverse();
        let chosen = client().select_payment_requirements(&reversed).unwrap();
        assert_eq!(chosen.max_amount_required.to_string(), "10000");
    }

    #[test]
    fn a_client_that_wants_evidence_pays_for_the_offer_that_carries_it() {
        let chosen = client()
            .prefer_durable_evidence()
            .select_payment_requirements(&pair())
            .unwrap();
        assert_eq!(chosen.max_amount_required.to_string(), "12000");
        assert_eq!(
            DurableEvidenceConfig::from_requirements(&chosen)
                .unwrap()
                .retention,
            Retention::Year1,
            "and gets the terms it paid for"
        );
    }

    #[test]
    fn preferring_evidence_never_invents_an_offer() {
        // If the seller only offers the plain resource, wanting evidence
        // changes nothing. The client still pays; it just gets no receipt.
        let only_plain = vec![offer("10000")];
        let chosen = client()
            .prefer_durable_evidence()
            .select_payment_requirements(&only_plain)
            .unwrap();
        assert_eq!(chosen.max_amount_required.to_string(), "10000");
    }
}
