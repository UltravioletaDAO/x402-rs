//! Core trait defining the verification and settlement interface for x402 facilitators.
//!
//! Implementors of this trait are responsible for validating incoming payment payloads
//! against specified requirements [`Facilitator::verify`] and executing on-chain transfers [`Facilitator::settle`].

use crate::types::{
    SettleRequest, SettleResponse, SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse,
};
use std::fmt::{Debug, Display};
use std::sync::Arc;

/// Trait defining the asynchronous interface for x402 payment facilitators.
///
/// This interface is implemented by any type that performs validation and
/// settlement of payment payloads according to the x402 specification.
pub trait Facilitator {
    /// The error type returned by this facilitator.
    type Error: Debug + Display;

    /// Verifies a proposed x402 payment payload against a [`VerifyRequest`].
    ///
    /// This includes checking payload integrity, signature validity, balance sufficiency,
    /// network compatibility, and compliance with the declared payment requirements.
    ///
    /// # Returns
    ///
    /// A [`VerifyResponse`] indicating success or failure, wrapped in a [`Result`].
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if any validation step fails.
    fn verify(
        &self,
        request: &VerifyRequest,
    ) -> impl Future<Output = Result<VerifyResponse, Self::Error>> + Send;

    /// Executes an on-chain x402 settlement for a valid [`SettleRequest`].
    ///
    /// This method should re-validate the payment and, if valid, perform
    /// an onchain call to settle the payment.
    ///
    /// # Returns
    ///
    /// A [`SettleResponse`] indicating whether the settlement was successful, and
    /// containing any on-chain transaction metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if verification or settlement fails.
    fn settle(
        &self,
        request: &SettleRequest,
    ) -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send;

    #[allow(dead_code)] // For some reason clippy believes it is not used.
    fn supported(
        &self,
    ) -> impl Future<Output = Result<SupportedPaymentKindsResponse, Self::Error>> + Send;

    /// Returns information about the current blacklist configuration.
    ///
    /// This method provides visibility into which addresses are blocked from
    /// using the facilitator. The default implementation returns an empty response.
    ///
    /// # Returns
    ///
    /// A JSON value containing blacklist statistics and entries.
    fn blacklist_info(
        &self,
    ) -> impl Future<Output = Result<serde_json::Value, Self::Error>> + Send {
        async {
            Ok(serde_json::json!({
                "total_blocked": 0,
                "evm_count": 0,
                "solana_count": 0,
                "entries": [],
                "source": "none",
                "loaded_at_startup": false
            }))
        }
    }
}

impl<T: Facilitator> Facilitator for Arc<T> {
    type Error = T::Error;

    fn verify(
        &self,
        request: &VerifyRequest,
    ) -> impl Future<Output = Result<VerifyResponse, Self::Error>> + Send {
        self.as_ref().verify(request)
    }

    fn settle(
        &self,
        request: &SettleRequest,
    ) -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send {
        self.as_ref().settle(request)
    }

    fn supported(
        &self,
    ) -> impl Future<Output = Result<SupportedPaymentKindsResponse, Self::Error>> + Send {
        self.as_ref().supported()
    }

    fn blacklist_info(
        &self,
    ) -> impl Future<Output = Result<serde_json::Value, Self::Error>> + Send {
        self.as_ref().blacklist_info()
    }
}
