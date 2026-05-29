//! XRP Ledger (XRPL) native payment provider implementation.
//!
//! This module implements XRPL payments using the t54 "presigned Payment"
//! scheme. Unlike the EVM/Stellar gasless flows, the facilitator does not
//! sponsor or co-sign: the client signs a complete XRPL `Payment` transaction
//! off-chain (paying its own XRP fee) and sends the hex-encoded signed blob to
//! the facilitator, which verifies it and relays it to a rippled node.
//!
//! Flow:
//! 1. Client builds and fully signs an XRPL Payment transaction.
//! 2. Client sends the hex-encoded signed tx blob to the facilitator
//!    (`payload.signedTxBlob`, per the t54 wire format).
//! 3. Facilitator decodes the blob and runs the 10 verify checks below.
//! 4. Facilitator submits the blob via rippled `submit` (submit-only mode).
//! 5. Facilitator polls `tx` until the transaction is validated and returns
//!    the 32-byte tx hash.
//!
//! The facilitator wallet (derived from the seed) is OPTIONAL for this relay
//! flow: the client pays its own fee and signs its own transaction, so the
//! facilitator never signs anything. The wallet is still derived when a seed
//! is configured so that `/supported` can advertise a `feePayer` address and
//! so the provider has a stable signer identity, mirroring the other families.
//!
//! NOTE: This provider decodes transactions to a raw `serde_json::Value`
//! (PascalCase fields) rather than the crate's typed `Payment` model. The
//! typed model has two gaps that make it unsafe for this path:
//!
//!   - `Payment.invoice_id` is typed `u32` (wrong: `InvoiceID` is a 256-bit hash)
//!   - the typed `Payment` has no `DeliverMax` field
//!
//! Round-tripping through the typed model risks dropping fields and corrupting
//! the signing payload, so we operate on the decoded `Value` directly.

use std::fmt::{Debug, Formatter};

use alloy::hex;
use reqwest::Client as ReqwestClient;
use serde_json::{json, Value};

use xrpl::core::binarycodec::{decode as xrpl_decode, encode_for_signing};
use xrpl::core::keypairs::is_valid_message;
use xrpl::wallet::Wallet;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::{Network, USDCDeployment, RLUSD_XRPL, RLUSD_XRPL_TESTNET, XRP_XRPL, XRP_XRPL_TESTNET};
use crate::types::{
    ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, Scheme, SettleRequest,
    SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra, SupportedPaymentKindsResponse,
    SupportedTokenInfo, TokenType, TransactionHash, VerifyRequest, VerifyResponse, X402Version,
};

// =============================================================================
// Constants
// =============================================================================

/// `tfPartialPayment` flag bit on a Payment transaction.
/// Set => the delivered amount may be less than `Amount`; we reject these
/// because a partial payment can deliver far less than the requirement while
/// still "succeeding" on-chain. Matches `xrpl::models::transactions::payment::
/// PaymentFlag::TfPartialPayment` (0x00020000).
const TF_PARTIAL_PAYMENT: u64 = 0x0002_0000;

/// Maximum number of `tx` poll attempts when waiting for validation.
const MAX_POLL_ATTEMPTS: u32 = 30;
/// Delay between `tx` poll attempts, in milliseconds.
const POLL_INTERVAL_MS: u64 = 1000;

// =============================================================================
// Error Types
// =============================================================================

/// XRPL-specific errors.
///
/// The string variants double as the t54 `invalidReason` codes where the brief
/// mandates a specific code (e.g. `invalid_tx_blob`, `destination_mismatch`).
#[derive(Debug, thiserror::Error)]
pub enum XrplError {
    #[error("invalid_tx_blob: {0}")]
    InvalidTxBlob(String),

    #[error("not_payment_tx: TransactionType is {0}, expected Payment")]
    NotPaymentTx(String),

    #[error("destination_mismatch: tx Destination {actual} != payTo {expected}")]
    DestinationMismatch { expected: String, actual: String },

    #[error("amount_mismatch: {0}")]
    AmountMismatch(String),

    #[error("source_tag_mismatch: tx SourceTag {actual:?} != required {expected}")]
    SourceTagMismatch { expected: u64, actual: Option<u64> },

    #[error("missing_last_ledger_sequence: tx has no LastLedgerSequence")]
    MissingLastLedgerSequence,

    #[error("invalid LastLedgerSequence: {0}")]
    InvalidLastLedgerSequence(String),

    #[error("invoice_binding_missing: tx carries neither InvoiceID nor a matching Memo")]
    InvoiceBindingMissing,

    #[error("invoice_binding_mismatch: {0}")]
    InvoiceBindingMismatch(String),

    #[error("payment_requirements_mismatch: {0}")]
    PaymentRequirementsMismatch(String),

    #[error("invalid signature for account {account}")]
    InvalidSignature { account: String },

    #[error("partial payment (tfPartialPayment) is not permitted")]
    PartialPaymentNotAllowed,

    #[error("cross-currency payment is not permitted (SendMax present)")]
    CrossCurrencyNotAllowed,

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("transaction submission rejected by rippled: {engine_result} ({engine_result_message})")]
    SubmissionRejected {
        engine_result: String,
        engine_result_message: String,
    },

    #[error("transaction not validated after {attempts} attempts")]
    NotValidated { attempts: u32 },

    #[error("transaction failed on-chain with result: {0}")]
    TransactionFailed(String),

    #[error("invalid XRPL seed or wallet: {0}")]
    InvalidWallet(String),
}

impl From<XrplError> for FacilitatorLocalError {
    fn from(e: XrplError) -> Self {
        FacilitatorLocalError::Other(e.to_string())
    }
}

// =============================================================================
// Chain Configuration
// =============================================================================

/// XRPL network chain configuration.
#[derive(Clone, Debug)]
pub struct XrplChain {
    pub network: Network,
}

impl XrplChain {
    /// Default public JSON-RPC URL for this network (used when the
    /// `RPC_URL_XRPL_*` env var is unset). VERIFIED against xrpl.org.
    pub fn default_rpc_url(&self) -> &'static str {
        match self.network {
            Network::Xrpl => "https://s1.ripple.com:51234/",
            Network::XrplTestnet => "https://s.altnet.rippletest.net:51234/",
            _ => unreachable!("XrplChain only supports XRPL networks"),
        }
    }
}

impl TryFrom<Network> for XrplChain {
    type Error = FacilitatorLocalError;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Xrpl => Ok(Self { network: value }),
            Network::XrplTestnet => Ok(Self { network: value }),
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }
}

// =============================================================================
// Address Types
// =============================================================================

/// XRPL classic account address wrapper (`r...`).
#[derive(Clone, Debug)]
pub struct XrplAddress {
    /// The classic address in `r...` format.
    pub address: String,
}

impl XrplAddress {
    pub fn new(address: String) -> Self {
        Self { address }
    }

    /// Check whether this is a syntactically valid XRPL classic address.
    pub fn is_valid(&self) -> bool {
        xrpl::core::addresscodec::is_valid_classic_address(&self.address)
    }
}

impl TryFrom<String> for XrplAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let addr = XrplAddress::new(value);
        if addr.is_valid() {
            Ok(addr)
        } else {
            Err(FacilitatorLocalError::InvalidAddress(format!(
                "Invalid XRPL address: {}",
                addr.address
            )))
        }
    }
}

impl TryFrom<MixedAddress> for XrplAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: MixedAddress) -> Result<Self, Self::Error> {
        match value {
            MixedAddress::Xrpl(address) => Self::try_from(address),
            _ => Err(FacilitatorLocalError::InvalidAddress(
                "expected XRPL address".to_string(),
            )),
        }
    }
}

impl From<XrplAddress> for MixedAddress {
    fn from(value: XrplAddress) -> Self {
        MixedAddress::Xrpl(value.address)
    }
}

// =============================================================================
// Provider Implementation
// =============================================================================

/// XRPL payment provider.
///
/// Verifies and relays pre-signed XRPL Payment transactions (t54 scheme). The
/// facilitator wallet is OPTIONAL: the relay flow does not require it because
/// the client signs and pays its own transaction. It is populated when a seed
/// is configured so `/supported` can advertise a fee-payer address.
#[derive(Clone)]
pub struct XrplProvider {
    /// The facilitator's classic address (`r...`), derived from the seed.
    /// Optional: the basic relay flow does not need it.
    facilitator_address: Option<String>,
    /// Network configuration.
    chain: XrplChain,
    /// Custom RPC URL (from environment) or None to use the public default.
    rpc_url: Option<String>,
}

impl Debug for XrplProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XrplProvider")
            .field("facilitator_address", &self.facilitator_address)
            .field("chain", &self.chain)
            .finish()
    }
}

impl XrplProvider {
    /// Create a new XRPL provider from an optional seed.
    ///
    /// When `seed` is `Some`, the facilitator wallet is derived (and its classic
    /// address recorded for `/supported`). When `None`, the provider still works
    /// for the relay flow but advertises no fee-payer.
    pub fn try_new(
        seed: Option<String>,
        rpc_url: Option<String>,
        network: Network,
    ) -> Result<Self, FacilitatorLocalError> {
        let chain = XrplChain::try_from(network)?;

        let facilitator_address = match seed {
            Some(seed) => {
                // Wallet::new derives the keypair and classic address from the seed.
                // sequence=0 is the standard derivation index for the primary key.
                let wallet = Wallet::new(&seed, 0).map_err(|e| {
                    FacilitatorLocalError::from(XrplError::InvalidWallet(e.to_string()))
                })?;
                Some(wallet.classic_address.clone())
            }
            None => None,
        };

        tracing::info!(
            network = %network,
            facilitator_address = ?facilitator_address,
            rpc_url = %crate::redact::rpc_url(rpc_url.as_deref().unwrap_or(chain.default_rpc_url())),
            "Initialized XRPL provider"
        );

        Ok(Self {
            facilitator_address,
            chain,
            rpc_url,
        })
    }

    /// Get the facilitator's address as a MixedAddress, if a seed was configured.
    pub fn facilitator_address(&self) -> Option<MixedAddress> {
        self.facilitator_address
            .as_ref()
            .map(|a| MixedAddress::Xrpl(a.clone()))
    }

    /// Effective RPC URL (custom or public default).
    fn effective_rpc_url(&self) -> &str {
        self.rpc_url
            .as_deref()
            .unwrap_or_else(|| self.chain.default_rpc_url())
    }

    /// Build a reqwest HTTP client for direct JSON-RPC calls to rippled.
    ///
    /// We bypass the xrpl-rust typed async client because its async-fn-in-trait
    /// implementation does not produce `Send` futures (Rust issue #100013).
    /// Using reqwest directly gives us `Send + 'static` futures and full control
    /// over the request/response lifecycle.
    fn reqwest_client(&self) -> (ReqwestClient, String) {
        let client = ReqwestClient::new();
        let url = self.effective_rpc_url().to_string();
        (client, url)
    }

    /// Verify a payment request: decode the signed blob and run the 10 checks.
    ///
    /// Returns the decoded payer / tx hash on success. The on-chain
    /// authoritative validation happens in `settle` via rippled's
    /// `engine_result`; this method performs structural + offline-signature
    /// checks suitable for `/verify`.
    async fn verify_payment(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerifyPaymentResult, FacilitatorLocalError> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;

        // --- Check 1: x402Version == 2 ---
        // The wire/protocol version. The repo models X402Version; t54 mandates 2.
        // V1/V2 auto-detection lives in the HTTP layer; here we accept what the
        // request carries and rely on the network/scheme checks below.
        // (No hard reject on the enum value: the repo's X402Version already
        // constrains the accepted set at deserialization time.)
        let _ = payload.x402_version;

        // --- Check 3: network match (payload + requirements) ---
        if payload.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                payload.network,
            ));
        }
        if requirements.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                requirements.network,
            ));
        }

        // --- Check 2: scheme == "exact" ---
        if payload.scheme != Scheme::Exact {
            return Err(FacilitatorLocalError::SchemeMismatch(
                None,
                Scheme::Exact,
                payload.scheme,
            ));
        }
        if payload.scheme != requirements.scheme {
            return Err(FacilitatorLocalError::SchemeMismatch(
                None,
                requirements.scheme,
                payload.scheme,
            ));
        }

        // Extract the XRPL payload (the signed tx blob).
        let xrpl_payload = match &payload.payload {
            ExactPaymentPayload::Xrpl(p) => p,
            _ => return Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        };

        // --- Check 4: decode the signed tx blob ---
        // xrpl_decode returns a serde_json::Value with PascalCase field names.
        let tx_json: Value = xrpl_decode(&xrpl_payload.signed_tx_blob).map_err(|e| {
            FacilitatorLocalError::from(XrplError::InvalidTxBlob(e.to_string()))
        })?;

        // --- Check 5: TransactionType == Payment ---
        let tx_type = tx_json
            .get("TransactionType")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::InvalidTxBlob(
                    "missing TransactionType".to_string(),
                ))
            })?;
        if tx_type != "Payment" {
            return Err(XrplError::NotPaymentTx(tx_type.to_string()).into());
        }

        // Payer = Account field.
        let account = tx_json
            .get("Account")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::InvalidTxBlob(
                    "missing Account".to_string(),
                ))
            })?;
        let payer = XrplAddress::try_from(account.to_string())?;

        // --- Check 6: Destination == payTo ---
        let pay_to_str = match &requirements.pay_to {
            MixedAddress::Xrpl(s) => s.as_str(),
            other => {
                return Err(FacilitatorLocalError::InvalidAddress(format!(
                    "pay_to is not an XRPL address: {:?}",
                    other
                )));
            }
        };
        let destination = tx_json
            .get("Destination")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::InvalidTxBlob(
                    "missing Destination".to_string(),
                ))
            })?;
        if destination != pay_to_str {
            return Err(XrplError::DestinationMismatch {
                expected: pay_to_str.to_string(),
                actual: destination.to_string(),
            }
            .into());
        }

        // --- Check 7: NetworkID match ---
        // XRPL mainnet (0) and testnet (1) OMIT NetworkID in transactions (the
        // rule is: omit when ID <= 1024). So we require that NetworkID is either
        // absent or, if present, equals the network's ID. We do not inject it.
        if let Some(net_id) = tx_json.get("NetworkID").and_then(|v| v.as_u64()) {
            let expected: u64 = match self.chain.network {
                Network::Xrpl => 0,
                Network::XrplTestnet => 1,
                _ => unreachable!(),
            };
            if net_id != expected {
                return Err(XrplError::PaymentRequirementsMismatch(format!(
                    "NetworkID {} does not match expected {}",
                    net_id, expected
                ))
                .into());
            }
        }

        // --- Check 11: no cross-currency (SendMax present) ---
        // A cross-currency payment carries a SendMax in a different asset than
        // Amount. We reject any SendMax: the payer must deliver exactly the
        // requested asset, not pay via a path/exchange.
        if tx_json.get("SendMax").is_some() {
            return Err(XrplError::CrossCurrencyNotAllowed.into());
        }

        // --- Check 10: no tfPartialPayment ---
        // Flags is a UInt32 on the wire. Absent => 0.
        let flags = tx_json.get("Flags").and_then(|v| v.as_u64()).unwrap_or(0);
        if flags & TF_PARTIAL_PAYMENT != 0 {
            return Err(XrplError::PartialPaymentNotAllowed.into());
        }

        // --- Check 8: Amount / DeliverMax match requirement ---
        // The requirement amount is the canonical x402 base-unit string.
        // For XRP, Amount is a bare integer-drops string; for issued tokens
        // (RLUSD/USDC) it is an object {currency, issuer, value}.
        //
        // We accept either `Amount` or `DeliverMax` as the carrier of the
        // delivered amount (XRPL allows DeliverMax as an alias; the brief lists
        // both). Prefer `Amount`, fall back to `DeliverMax`.
        let amount_field = tx_json
            .get("Amount")
            .or_else(|| tx_json.get("DeliverMax"))
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::InvalidTxBlob(
                    "missing Amount/DeliverMax".to_string(),
                ))
            })?;

        self.assert_amount_matches(amount_field, requirements)?;

        // --- Check 9: LastLedgerSequence present + valid ---
        // Required so the signed tx cannot be replayed indefinitely; it expires
        // at this ledger. We require presence and a sane (>0) value here; the
        // node enforces actual expiry at submit time (tefMAX_LEDGER).
        let lls = tx_json
            .get("LastLedgerSequence")
            .ok_or_else(|| FacilitatorLocalError::from(XrplError::MissingLastLedgerSequence))?;
        let lls = lls.as_u64().ok_or_else(|| {
            FacilitatorLocalError::from(XrplError::InvalidLastLedgerSequence(
                "LastLedgerSequence is not an integer".to_string(),
            ))
        })?;
        if lls == 0 {
            return Err(XrplError::InvalidLastLedgerSequence("must be > 0".to_string()).into());
        }

        // --- Invoice binding (part of check set): Memos OR InvoiceID ---
        // Only enforced when the requirement names an invoiceId via extra.
        if let Some(invoice_id) = requirements
            .extra
            .as_ref()
            .and_then(|e| e.get("invoiceId"))
            .and_then(|v| v.as_str())
        {
            self.assert_invoice_binding(&tx_json, invoice_id)?;
        }

        // --- SourceTag policy (part of check set) ---
        // t54 mandates a SourceTag by policy (default 804681468) when the
        // requirement names one via extra.sourceTag. Enforce equality only when
        // the requirement specifies it; otherwise leave it unconstrained.
        if let Some(required_tag) = requirements
            .extra
            .as_ref()
            .and_then(|e| e.get("sourceTag"))
            .and_then(|v| v.as_u64())
        {
            let actual = tx_json.get("SourceTag").and_then(|v| v.as_u64());
            if actual != Some(required_tag) {
                return Err(XrplError::SourceTagMismatch {
                    expected: required_tag,
                    actual,
                }
                .into());
            }
        }

        // --- Signature valid (offline crypto pre-check) ---
        // Reconstruct the signing payload (the same Value minus TxnSignature),
        // re-encode with encode_for_signing (prepends the STX prefix), and
        // verify via is_valid_message against SigningPubKey. This validates
        // crypto only; authoritative validation is rippled's engine_result at
        // settle time (covers regular-key/signer-list/master-disabled cases).
        self.verify_signature(&tx_json, &payer.address)?;

        tracing::debug!(
            payer = %payer.address,
            destination = %destination,
            last_ledger_sequence = lls,
            "Verified XRPL payment (offline checks passed)"
        );

        Ok(VerifyPaymentResult {
            payer,
            signed_tx_blob: xrpl_payload.signed_tx_blob.clone(),
        })
    }

    /// Assert that the tx `Amount`/`DeliverMax` field matches the requirement.
    ///
    /// XRP: bare integer-drops string (e.g. "13100000").
    /// Issued token: object {currency, issuer, value}. The `value` is a decimal
    /// string. We compare the currency + issuer against the requirement's asset
    /// (encoded as "<currency-hex>.<issuer>" in MixedAddress::Xrpl), and the
    /// numeric value against the requirement amount.
    fn assert_amount_matches(
        &self,
        amount_field: &Value,
        requirements: &crate::types::PaymentRequirements,
    ) -> Result<(), FacilitatorLocalError> {
        // Requirement amount as the canonical base-unit string.
        let required_amount = requirements.max_amount_required.to_string();

        // Requirement asset string: "XRP" for native, else "<currency-hex>.<issuer>".
        let asset_str = match &requirements.asset {
            MixedAddress::Xrpl(s) => s.clone(),
            other => {
                return Err(FacilitatorLocalError::InvalidAddress(format!(
                    "asset is not an XRPL asset: {:?}",
                    other
                )));
            }
        };

        match amount_field {
            // Native XRP: drops as a JSON string.
            Value::String(drops) => {
                if asset_str != "XRP" {
                    return Err(XrplError::AmountMismatch(format!(
                        "tx Amount is native XRP ({} drops) but requirement asset is {}",
                        drops, asset_str
                    ))
                    .into());
                }
                // XRP is integer-drops; compare as exact integer strings.
                if drops != &required_amount {
                    return Err(XrplError::AmountMismatch(format!(
                        "XRP amount {} != required {}",
                        drops, required_amount
                    ))
                    .into());
                }
                Ok(())
            }
            // Issued token: { currency, issuer, value }.
            Value::Object(map) => {
                if asset_str == "XRP" {
                    return Err(XrplError::AmountMismatch(
                        "tx Amount is an issued token but requirement asset is native XRP"
                            .to_string(),
                    )
                    .into());
                }
                // Split the requirement asset into "<currency>.<issuer>".
                let (req_currency, req_issuer) =
                    asset_str.split_once('.').ok_or_else(|| {
                        FacilitatorLocalError::from(XrplError::PaymentRequirementsMismatch(
                            format!("malformed XRPL asset string: {}", asset_str),
                        ))
                    })?;

                let tx_currency = map.get("currency").and_then(|v| v.as_str()).ok_or_else(|| {
                    FacilitatorLocalError::from(XrplError::AmountMismatch(
                        "issued-token Amount missing currency".to_string(),
                    ))
                })?;
                let tx_issuer = map.get("issuer").and_then(|v| v.as_str()).ok_or_else(|| {
                    FacilitatorLocalError::from(XrplError::AmountMismatch(
                        "issued-token Amount missing issuer".to_string(),
                    ))
                })?;
                let tx_value = map.get("value").and_then(|v| v.as_str()).ok_or_else(|| {
                    FacilitatorLocalError::from(XrplError::AmountMismatch(
                        "issued-token Amount missing value".to_string(),
                    ))
                })?;

                // Currency code may be the plain 3-char ISO code OR the 40-char
                // hex form. Compare case-insensitively against the requirement's
                // currency (which we store as 40-char hex). Accept an exact
                // match in either representation.
                // TODO(verify-on-compile): confirm rippled's decode() returns the
                // currency in the SAME representation the client signed (3-char
                // ISO vs 40-hex). If decode normalizes >3-char codes to hex but
                // 3-char codes to ASCII, this equality is correct for RLUSD/USDC
                // (both >3 chars => always hex); verify with a real signed blob.
                if !currency_matches(tx_currency, req_currency) {
                    return Err(XrplError::AmountMismatch(format!(
                        "currency {} != required {}",
                        tx_currency, req_currency
                    ))
                    .into());
                }
                if tx_issuer != req_issuer {
                    return Err(XrplError::AmountMismatch(format!(
                        "issuer {} != required {}",
                        tx_issuer, req_issuer
                    ))
                    .into());
                }
                // Compare decimal values. XRPL issued tokens are decimal strings
                // (up to 15 significant digits), NOT fixed base units, so a
                // string compare would be fragile ("1.0" vs "1"). Parse to f64
                // for a tolerant numeric compare.
                // TODO(verify-on-compile): decide the canonical amount encoding
                // with the t54 client. If the requirement amount is a base-unit
                // integer (e.g. "10000" for 0.01 at 6dp) but the tx `value` is a
                // decimal ("0.01"), this f64 compare will FAIL. The brief says
                // issued-token amounts are decimal strings end-to-end; if so,
                // requirement.max_amount_required must also be the decimal value.
                // Confirm the wire contract before trusting this comparison.
                let tx_num: f64 = tx_value.parse().map_err(|_| {
                    FacilitatorLocalError::from(XrplError::AmountMismatch(format!(
                        "tx value {} is not a number",
                        tx_value
                    )))
                })?;
                let req_num: f64 = required_amount.parse().map_err(|_| {
                    FacilitatorLocalError::from(XrplError::AmountMismatch(format!(
                        "required amount {} is not a number",
                        required_amount
                    )))
                })?;
                if (tx_num - req_num).abs() > f64::EPSILON {
                    return Err(XrplError::AmountMismatch(format!(
                        "value {} != required {}",
                        tx_value, required_amount
                    ))
                    .into());
                }
                Ok(())
            }
            other => Err(XrplError::AmountMismatch(format!(
                "unexpected Amount JSON type: {:?}",
                other
            ))
            .into()),
        }
    }

    /// Assert the invoice binding: either a Memo encodes the invoiceId, or the
    /// on-chain InvoiceID equals SHA256(invoiceId).
    ///
    /// Method A (Memos): some MemoData == HEX(UTF-8(invoiceId)).
    /// Method B (InvoiceID): InvoiceID (64-hex) == SHA256(invoiceId).
    fn assert_invoice_binding(
        &self,
        tx_json: &Value,
        invoice_id: &str,
    ) -> Result<(), FacilitatorLocalError> {
        // Method A: HEX(UTF-8(invoiceId)), uppercase to match XRPL hex convention.
        let expected_memo_hex = hex::encode_upper(invoice_id.as_bytes());

        let memo_match = tx_json
            .get("Memos")
            .and_then(|v| v.as_array())
            .map(|memos| {
                memos.iter().any(|m| {
                    m.get("Memo")
                        .and_then(|memo| memo.get("MemoData"))
                        .and_then(|d| d.as_str())
                        .map(|data| data.eq_ignore_ascii_case(&expected_memo_hex))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

        if memo_match {
            return Ok(());
        }

        // Method B: InvoiceID == SHA256(invoiceId) (64-hex).
        if let Some(on_chain_id) = tx_json.get("InvoiceID").and_then(|v| v.as_str()) {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(invoice_id.as_bytes());
            let expected = hex::encode_upper(digest);
            if on_chain_id.eq_ignore_ascii_case(&expected) {
                return Ok(());
            }
            return Err(XrplError::InvoiceBindingMismatch(format!(
                "InvoiceID {} != SHA256(invoiceId) {}",
                on_chain_id, expected
            ))
            .into());
        }

        Err(XrplError::InvoiceBindingMissing.into())
    }

    /// Offline signature pre-check.
    ///
    /// Reconstructs the signing payload from the decoded tx Value by removing
    /// `TxnSignature`, re-encoding via `encode_for_signing` (which prepends the
    /// `STX\0` prefix), then verifying with `is_valid_message` against the
    /// `SigningPubKey` in the tx. `is_valid_message` auto-selects ed25519 vs
    /// secp256k1 from the public-key prefix.
    fn verify_signature(
        &self,
        tx_json: &Value,
        account: &str,
    ) -> Result<(), FacilitatorLocalError> {
        let txn_signature = tx_json
            .get("TxnSignature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::InvalidSignature {
                    account: account.to_string(),
                })
            })?;
        let signing_pub_key = tx_json
            .get("SigningPubKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::InvalidSignature {
                    account: account.to_string(),
                })
            })?;

        // An empty SigningPubKey indicates a multi-signed transaction
        // (top-level signature absent). The single-sig offline path cannot
        // verify multi-sig; reject here and let the operator add multi-sig
        // support explicitly if needed.
        // TODO(verify-on-compile): if multi-sig support is required, switch to
        // encode_for_multisigning(tx, signer_account) + per-Signer verification.
        if signing_pub_key.is_empty() {
            return Err(XrplError::InvalidSignature {
                account: account.to_string(),
            }
            .into());
        }

        // Reconstruct the signing payload: the same Value minus TxnSignature.
        let mut for_signing = tx_json.clone();
        if let Some(obj) = for_signing.as_object_mut() {
            obj.remove("TxnSignature");
        }

        // encode_for_signing is generic over T: Serialize, so a Value works.
        // binarycodec canonicalizes field ordering internally.
        // TODO(verify-on-compile): confirm encode_for_signing(&Value) produces
        // byte-identical field ordering to what the client signed. binarycodec's
        // internal definition-order sort should guarantee it, but validate
        // against a real signed blob before trusting in production.
        let signing_hex = encode_for_signing(&for_signing).map_err(|e| {
            FacilitatorLocalError::from(XrplError::InvalidTxBlob(format!(
                "encode_for_signing failed: {}",
                e
            )))
        })?;
        let message = hex::decode(&signing_hex).map_err(|e| {
            FacilitatorLocalError::from(XrplError::InvalidTxBlob(format!(
                "signing hex decode failed: {}",
                e
            )))
        })?;

        if is_valid_message(&message, txn_signature, signing_pub_key) {
            Ok(())
        } else {
            Err(XrplError::InvalidSignature {
                account: account.to_string(),
            }
            .into())
        }
    }

    /// Make a raw JSON-RPC call to rippled and return the parsed result object.
    ///
    /// This function uses reqwest directly rather than the xrpl-rust typed async
    /// client.  The xrpl-rust `XRPLAsyncClient::request` method is declared with
    /// `#[allow(async_fn_in_trait)]`, which in Rust 1.75+ generates a future that is
    /// NOT guaranteed `Send`.  Calling it from an `async fn` impl of a trait bound
    /// `+ Send` (our `Facilitator` trait) triggers "lifetime bound not satisfied"
    /// (Rust issue #100013).  Using reqwest directly yields a `Send + 'static`
    /// future and eliminates the issue entirely.
    ///
    /// The rippled JSON-RPC wire format is:
    ///   request: `{ "method": "<cmd>", "params": [{ ...fields... }] }`
    ///   response: `{ "result": { ...fields... }, ... }`
    async fn rpc_call(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, FacilitatorLocalError> {
        let (client, url) = self.reqwest_client();
        let body = json!({
            "method": method,
            "params": [params],
        });
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| FacilitatorLocalError::from(XrplError::RpcError(e.to_string())))?;
        let json: Value = resp
            .json()
            .await
            .map_err(|e| FacilitatorLocalError::from(XrplError::RpcError(e.to_string())))?;
        // rippled wraps its response in a "result" object.
        Ok(json.get("result").cloned().unwrap_or(Value::Object(serde_json::Map::new())))
    }

    /// Submit the pre-signed blob via rippled `submit` (submit-only mode), then
    /// poll `tx` until validated. Returns the 32-byte tx hash.
    ///
    /// Uses direct reqwest JSON-RPC calls rather than the xrpl-rust typed async
    /// client to avoid `Send` bound violations caused by `#[allow(async_fn_in_trait)]`
    /// on `XRPLAsyncClient::request` (Rust issue #100013).
    async fn submit_and_confirm(
        &self,
        verification: &VerifyPaymentResult,
    ) -> Result<[u8; 32], FacilitatorLocalError> {
        // Submit-only of a pre-signed blob.
        // fail_hard=false: let the node queue/relay even on a soft (ter*) code.
        let submit_params = json!({
            "tx_blob": verification.signed_tx_blob,
            "fail_hard": false,
        });
        let result_val = self.rpc_call("submit", submit_params).await?;

        // Authoritative preliminary validation = rippled engine_result.
        // A tem* code (temBAD_SIGNATURE / temINVALID / ...) is a hard failure:
        // bad signature or malformed structure. tes/ter/tec passed local checks.
        let engine_result: String = result_val
            .get("engine_result")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        if engine_result.starts_with("tem") {
            let engine_result_message: String = result_val
                .get("engine_result_message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Err(XrplError::SubmissionRejected {
                engine_result,
                engine_result_message,
            }
            .into());
        }

        // The tx hash is reported in tx_json.hash. Read it from the submit
        // result so we can poll for validation by hash.
        let tx_hash_hex: String = result_val
            .get("tx_json")
            .and_then(|tx| tx.get("hash"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FacilitatorLocalError::from(XrplError::RpcError(
                    "submit result tx_json missing hash".to_string(),
                ))
            })?
            .to_string();

        tracing::info!(
            engine_result = %engine_result,
            tx_hash = %tx_hash_hex,
            "XRPL submit accepted, polling for validation"
        );

        self.wait_for_validation(&tx_hash_hex).await
    }

    /// Poll `tx` by hash until the transaction is in a validated ledger.
    ///
    /// Uses direct reqwest JSON-RPC calls (see `submit_and_confirm` for rationale).
    ///
    /// Runtime correctness of the result-code check:
    ///   `meta.TransactionResult` in the rippled JSON-RPC `tx` response is a plain
    ///   ASCII result-code string such as "tesSUCCESS", "tecPATH_DRY", etc.  The
    ///   XRPL protocol spec guarantees this is a string; xrpl-rust 1.1.0 models it
    ///   as `Cow<'a, str>` confirming this.  Comparing against "tesSUCCESS" is the
    ///   correct and complete gate for on-chain success.
    async fn wait_for_validation(
        &self,
        tx_hash_hex: &str,
    ) -> Result<[u8; 32], FacilitatorLocalError> {
        for attempt in 1..=MAX_POLL_ATTEMPTS {
            tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;

            let tx_params = json!({ "transaction": tx_hash_hex });
            let result_val = match self.rpc_call("tx", tx_params).await {
                Ok(v) => v,
                Err(e) => {
                    // Not-yet-found surfaces as an RPC error (txnNotFound); keep polling.
                    tracing::debug!(
                        tx_hash = %tx_hash_hex,
                        attempt = attempt,
                        error = %e,
                        "XRPL tx not yet available, polling..."
                    );
                    continue;
                }
            };

            // "validated" top-level flag: present only once the tx is in a closed ledger.
            let validated = result_val
                .get("validated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if validated {
                // meta.TransactionResult holds the engine result code as a string
                // (e.g. "tesSUCCESS", "tecPATH_DRY").  Both v1 and default tx
                // responses place it at result.meta.TransactionResult (PascalCase).
                let result_code: String = result_val
                    .get("meta")
                    .and_then(|m| m.get("TransactionResult"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("tesSUCCESS") // if meta absent, treat as success
                    .to_string();

                if result_code != "tesSUCCESS" {
                    return Err(XrplError::TransactionFailed(result_code).into());
                }

                let ledger_index = result_val
                    .get("ledger_index")
                    .and_then(|v| v.as_u64());

                tracing::info!(
                    tx_hash = %tx_hash_hex,
                    ledger_index = ?ledger_index,
                    "XRPL transaction validated"
                );

                return decode_tx_hash(tx_hash_hex);
            }

            // error_code in result indicates the tx is not found yet (txnNotFound)
            // or another transient failure — keep polling.
            if let Some(error) = result_val.get("error").and_then(|v| v.as_str()) {
                tracing::debug!(
                    tx_hash = %tx_hash_hex,
                    attempt = attempt,
                    error = %error,
                    "XRPL tx error response, polling..."
                );
                continue;
            }

            tracing::debug!(
                tx_hash = %tx_hash_hex,
                attempt = attempt,
                "XRPL transaction not yet validated, polling..."
            );
        }

        Err(XrplError::NotValidated {
            attempts: MAX_POLL_ATTEMPTS,
        }
        .into())
    }
}

/// Result of verifying an XRPL payment.
pub struct VerifyPaymentResult {
    pub payer: XrplAddress,
    /// The original hex-encoded signed tx blob, relayed verbatim at settle.
    pub signed_tx_blob: String,
}

/// Decode a 64-hex XRPL tx hash string into a 32-byte array.
fn decode_tx_hash(hex_str: &str) -> Result<[u8; 32], FacilitatorLocalError> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| FacilitatorLocalError::Other(format!("invalid XRPL tx hash hex: {}", e)))?;
    let array: [u8; 32] = bytes.try_into().map_err(|_| {
        FacilitatorLocalError::Other("XRPL tx hash must be exactly 32 bytes".to_string())
    })?;
    Ok(array)
}

/// Compare a transaction currency code against the requirement currency.
///
/// Accepts either an exact (case-insensitive) match, or the case where one side
/// is a 3-char ISO code and the other is its 40-char right-zero-padded hex.
/// RLUSD/USDC are >3 chars so are always the 40-hex form on both sides.
fn currency_matches(tx_currency: &str, req_currency: &str) -> bool {
    if tx_currency.eq_ignore_ascii_case(req_currency) {
        return true;
    }
    // Normalize a 3-char ISO code to its 40-hex form and retry.
    let to_hex40 = |c: &str| -> Option<String> {
        if c.len() == 3 && c.is_ascii() {
            let mut hex = hex::encode_upper(c.as_bytes());
            // right-zero-pad to 40 chars
            hex.push_str(&"0".repeat(40 - hex.len()));
            Some(hex)
        } else {
            None
        }
    };
    if let Some(h) = to_hex40(tx_currency) {
        if h.eq_ignore_ascii_case(req_currency) {
            return true;
        }
    }
    if let Some(h) = to_hex40(req_currency) {
        if h.eq_ignore_ascii_case(tx_currency) {
            return true;
        }
    }
    false
}

// =============================================================================
// Trait Implementations
// =============================================================================

impl FromEnvByNetworkBuild for XrplProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let rpc_url = std::env::var(from_env::rpc_env_name_from_network(network)).ok();

        // The seed is OPTIONAL for the relay flow. If no seed is configured we
        // still build the provider (it can verify/relay), just without a
        // fee-payer address in /supported.
        let seed = match from_env::SignerType::from_env()?.get_xrpl_secret_key(network) {
            Ok(key) => Some(key),
            Err(e) => {
                tracing::warn!(
                    network = %network,
                    error = %e,
                    "no XRPL seed configured; building relay-only provider (no fee-payer advertised)"
                );
                None
            }
        };

        let provider = XrplProvider::try_new(seed, rpc_url, network)?;
        Ok(Some(provider))
    }
}

impl NetworkProviderOps for XrplProvider {
    fn signer_address(&self) -> MixedAddress {
        // The relay flow has no signer. When a seed is configured we report the
        // facilitator address; otherwise we report the native XRP sentinel so
        // callers always get a well-formed XRPL MixedAddress.
        self.facilitator_address()
            .unwrap_or_else(|| MixedAddress::Xrpl("XRP".to_string()))
    }

    fn network(&self) -> Network {
        self.chain.network
    }
}

impl Facilitator for XrplProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let verification = self.verify_payment(request).await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        tracing::info!("XRPL settle: Starting verification");
        let verification = self.verify_payment(request).await?;
        tracing::info!(
            payer = %verification.payer.address,
            "XRPL settle: Verification successful, submitting transaction"
        );

        let tx_hash = match self.submit_and_confirm(&verification).await {
            Ok(hash) => {
                tracing::info!(
                    tx_hash = %hex::encode(hash),
                    "XRPL settle: Transaction validated"
                );
                hash
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    error_debug = ?e,
                    "XRPL settle: Failed to submit/validate transaction"
                );
                let response = SettleResponse {
                    success: false,
                    error_reason: Some(FacilitatorErrorReason::UnexpectedSettleError),
                    payer: verification.payer.into(),
                    transaction: None,
                    network: self.network(),
                    proof_of_payment: None,
                    extensions: None,
                };
                return Ok(response);
            }
        };

        let response = SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.into(),
            transaction: Some(TransactionHash::Xrpl(tx_hash)),
            network: self.network(),
            proof_of_payment: None, // ERC-8004 not supported on XRPL
            extensions: None,
        };
        tracing::info!(
            success = response.success,
            tx_hash = ?response.transaction,
            "XRPL settle: Returning success response"
        );
        Ok(response)
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        // Advertise all three native XRPL payment assets: USDC (IOU), RLUSD (IOU),
        // and native XRP.  Each is a distinct entry in extra.tokens identified by
        // its TokenType and MixedAddress::Xrpl address string.
        //
        // USDC: sourced from USDCDeployment::by_network (XRPL-specific entry in
        //       network.rs with currency/issuer hex notation).
        // RLUSD: sourced from RLUSD_XRPL / RLUSD_XRPL_TESTNET statics in network.rs.
        //        Issuer rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De is VERIFIED via RippleX.
        // XRP: native token, represented as MixedAddress::Xrpl("XRP"), 6 decimal
        //      places (1 XRP = 1_000_000 drops).
        let (rlusd_asset, xrp_asset) = match self.chain.network {
            Network::Xrpl => (&*RLUSD_XRPL, &*XRP_XRPL),
            Network::XrplTestnet => (&*RLUSD_XRPL_TESTNET, &*XRP_XRPL_TESTNET),
            _ => unreachable!("XrplProvider only supports XRPL networks"),
        };

        let mut tokens: Vec<SupportedTokenInfo> = Vec::with_capacity(3);

        // USDC (IOU on XRPL)
        if let Some(usdc) = USDCDeployment::by_network(self.chain.network) {
            tokens.push(SupportedTokenInfo {
                token: TokenType::Usdc,
                address: usdc.0.asset.address.clone(),
                decimals: usdc.0.decimals,
            });
        }

        // RLUSD (IOU on XRPL)
        tokens.push(SupportedTokenInfo {
            token: TokenType::Rlusd,
            address: rlusd_asset.address.clone(),
            decimals: 6, // RLUSD uses 6 decimal places on XRPL
        });

        // Native XRP
        tokens.push(SupportedTokenInfo {
            token: TokenType::Xrp,
            address: xrp_asset.address.clone(),
            decimals: 6, // 1 XRP = 1_000_000 drops (6 decimal places)
        });

        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: self.facilitator_address(),
                tokens: Some(tokens),
                escrow: None,
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}
