//! Price semantics for the Bazaar catalog.
//!
//! # Why this module exists
//!
//! The catalog and the payment path used to share one type,
//! [`PaymentRequirementsV2`]. That type is a *protocol* type: its `scheme` is
//! the closed [`Scheme`] enum, because a payment we cannot name is a payment we
//! cannot make. A *catalog* has the opposite obligation — it has to be able to
//! carry an offer it cannot settle, and say so, rather than relabel it as
//! something it can.
//!
//! Sharing the type forced three losses at the import boundary, all of them
//! measured against the live Coinbase CDP feed on 2026-09-10:
//!
//! | Loss | Measured |
//! |---|---|
//! | every scheme rewritten to `exact` | 27 of 178 accepts entries on the first CDP page were `batch-settlement` or `agent-pay` |
//! | `extra` dropped | 178 of 178 carried one (`name`/`version`, `receiverAuthorizer`, `withdrawDelay`, …) |
//! | an unparseable amount became `0` | 3 of 178 declared a *decimal* amount (`"0.002"`) in a field that holds atomic units |
//!
//! The third is the dangerous one: `U256::from_str("0.002")` fails, the old
//! converter mapped that failure to `TokenAmount::from(0u64)`, and a catalog
//! entry that says zero says *free*.
//!
//! [`CatalogPaymentOption`] is therefore the catalog's own record of one
//! declared payment option. It serialises to the same JSON as
//! [`PaymentRequirementsV2`] for every scheme we recognise, so no consumer of
//! `GET /discovery/resources` has to change; an unrecognised scheme now travels
//! as the string the source actually published.
//!
//! # Invariants
//!
//! - An unrecognised scheme is preserved verbatim and marked not settleable. It
//!   is **never** rewritten to `exact`.
//! - An absent, malformed, negative or oversized amount **rejects the option**,
//!   with a classified reason. It never becomes zero.
//! - An amount the source declared as `"0"` is an explicit zero and is kept as
//!   one. Whether a free listing is publishable stays a catalog policy decision
//!   (`curation_check`'s `unpayable` rule), not a parse outcome.
//! - Amounts stay integers in atomic units end to end. Nothing here converts one
//!   to a float.
//! - "Can we catalog it" and "can we settle it" are different questions.
//!   [`CatalogPaymentOption::annotate`] answers the second one separately.

use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::caip2::Caip2NetworkId;
use crate::network::{get_token_deployment, Network};
use crate::types::{MixedAddress, Scheme, TokenAmount, TokenType};
use crate::types_v2::PaymentRequirementsV2;

/// Longest scheme identifier we will carry. Long enough for every scheme in the
/// x402 specs repository plus room for a vendor prefix; short enough that a feed
/// cannot use the field as a payload.
pub const MAX_SCHEME_LEN: usize = 64;

/// Longest serialized `extra` we will persist per payment option.
///
/// Measured on the CDP feed (2026-09-10): p50 59 bytes, p90 121, max 720 — the
/// max being a JWT quote token. 8 KiB is an order of magnitude above the
/// observed ceiling and still bounds what one feed can push into our S3
/// snapshot, which is written whole on every import cycle.
pub const MAX_EXTRA_BYTES: usize = 8 * 1024;

/// Longest serialized resource-level `extensions` blob we will persist.
///
/// Measured on the same feed: p50 1.8 KiB, p90 3.7 KiB, max 8.4 KiB — mostly the
/// `bazaar` extension's input/output JSON Schema, which is what tells a buyer
/// *what* they are buying and so is load-bearing for comparing two prices.
pub const MAX_EXTENSIONS_BYTES: usize = 16 * 1024;

/// Deepest resource-level `extensions` we will persist.
///
/// Far above [`crate::json_depth::MAX_EXTRA_JSON_DEPTH`], and it has to be: that
/// bound (16) is calibrated for a payment `extra`, which is `{name, version}` in
/// practice. A JSON Schema is a different kind of document — the live Coinbase
/// feed publishes one **26 levels deep** — and applying the payment bound to it
/// rejected the entire page. Still far below `serde_json`'s own 128-level
/// recursion limit, which is what actually bounds the parse.
pub const MAX_EXTENSIONS_DEPTH: usize = 64;

/// Keep a payment option's `extra` if it is within bounds, else drop it.
///
/// Dropping beats truncating (half a JSON document is not a smaller one) and
/// beats failing (an oversized `extra` still leaves a usable listing, and a
/// third party's decoration must not be able to reject a page of prices).
pub fn sanitize_extra(extra: Option<serde_json::Value>) -> Option<serde_json::Value> {
    within(
        extra,
        crate::json_depth::MAX_EXTRA_JSON_DEPTH,
        MAX_EXTRA_BYTES,
    )
}

/// The same, for a resource-level `extensions` blob, at its own bounds.
pub fn sanitize_extensions(ext: Option<serde_json::Value>) -> Option<serde_json::Value> {
    within(ext, MAX_EXTENSIONS_DEPTH, MAX_EXTENSIONS_BYTES)
}

fn within(
    v: Option<serde_json::Value>,
    max_depth: usize,
    max_bytes: usize,
) -> Option<serde_json::Value> {
    let value = v?;
    if crate::json_depth::json_value_depth(&value) > max_depth {
        return None;
    }
    match serde_json::to_string(&value) {
        Ok(s) if s.len() <= max_bytes => Some(value),
        _ => None,
    }
}

/// `serde` shim for a third party's `extra`: out-of-bounds is dropped, never an
/// error. One hostile or merely verbose entry must not fail the whole page.
pub fn deserialize_tolerant_extra<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(sanitize_extra(Option::<serde_json::Value>::deserialize(
        deserializer,
    )?))
}

/// The same, for a resource-level `extensions` blob.
pub fn deserialize_tolerant_extensions<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(sanitize_extensions(
        Option::<serde_json::Value>::deserialize(deserializer)?,
    ))
}

/// Default `maxTimeoutSeconds` for a source that declares none.
const DEFAULT_MAX_TIMEOUT_SECS: u64 = 300;

// ============================================================================
// Scheme
// ============================================================================

/// A payment scheme as it appears in the catalog.
///
/// [`Scheme`] is closed on purpose — the settlement path must not accept a name
/// it cannot act on. The catalog needs the opposite: an offer whose scheme we do
/// not implement is still a real offer, and publishing it as `exact` is a
/// statement about someone else's money that we have no basis for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogScheme {
    /// A scheme this build implements.
    Known(Scheme),
    /// A scheme this build does not implement, preserved exactly as published.
    Unsupported(String),
}

impl CatalogScheme {
    /// Parse a scheme identifier. Recognised names map to [`Scheme`]; anything
    /// else that looks like an identifier is preserved verbatim.
    ///
    /// Returns `None` only for input that is not a scheme identifier at all
    /// (empty, over [`MAX_SCHEME_LEN`], or carrying characters no x402 scheme
    /// name uses) — a source that sends those has sent us a field we cannot
    /// interpret, and guessing is the failure mode this module exists to stop.
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim();
        if s.is_empty() || s.len() > MAX_SCHEME_LEN {
            return None;
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return None;
        }
        // Scheme names are lowercase in every x402 spec; match case-insensitively
        // so a feed shouting `EXACT` is still the scheme we implement, but keep
        // the source's own bytes for anything we do not recognise.
        Some(match s.to_ascii_lowercase().as_str() {
            "exact" => CatalogScheme::Known(Scheme::Exact),
            "upto" => CatalogScheme::Known(Scheme::Upto),
            "escrow" => CatalogScheme::Known(Scheme::Escrow),
            "commerce" => CatalogScheme::Known(Scheme::Commerce),
            "fhe-transfer" => CatalogScheme::Known(Scheme::FheTransfer),
            _ => CatalogScheme::Unsupported(s.to_string()),
        })
    }

    /// The [`Scheme`] this option can be settled with, if any.
    pub fn as_known(&self) -> Option<Scheme> {
        match self {
            CatalogScheme::Known(s) => Some(*s),
            CatalogScheme::Unsupported(_) => None,
        }
    }
}

impl std::fmt::Display for CatalogScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogScheme::Known(s) => write!(f, "{s}"),
            CatalogScheme::Unsupported(s) => write!(f, "{s}"),
        }
    }
}

impl From<Scheme> for CatalogScheme {
    fn from(s: Scheme) -> Self {
        CatalogScheme::Known(s)
    }
}

impl Serialize for CatalogScheme {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CatalogScheme {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        CatalogScheme::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid payment scheme: {s:?}")))
    }
}

// ============================================================================
// Amounts
// ============================================================================

/// Why a declared amount was refused.
///
/// Every variant means "we do not know what this costs". None of them means
/// zero: a catalog entry that says zero says *free*, and no source said that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountReject {
    /// The source declared no amount at all.
    Missing,
    /// Present but not an integer in atomic units — a decimal (`"0.002"`),
    /// scientific notation, a hex literal, or free text.
    NotAnInteger,
    /// Signed negative.
    Negative,
    /// A non-negative integer that does not fit in a `U256`.
    Overflow,
    /// `amount` and `maxAmountRequired` were both present and disagreed. Two
    /// prices are not a range; we do not know which one the seller meant.
    Conflicting,
}

impl AmountReject {
    /// Kebab-case identifier for logs and metrics. Bounded vocabulary: these
    /// end up as label values and must never carry source text.
    pub fn rule(self) -> &'static str {
        match self {
            AmountReject::Missing => "amount-missing",
            AmountReject::NotAnInteger => "amount-not-an-integer",
            AmountReject::Negative => "amount-negative",
            AmountReject::Overflow => "amount-overflow",
            AmountReject::Conflicting => "amount-conflicting",
        }
    }
}

impl std::fmt::Display for AmountReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.rule())
    }
}

/// Parse a declared amount in the asset's atomic units.
///
/// Deliberately stricter than [`TokenAmount`]'s own deserializer, which delegates
/// to `U256::from_str` and so accepts `0x`-prefixed hex. An amount field in x402
/// is a decimal integer string; accepting a second spelling of the same number
/// means two readers can disagree about what a listing costs.
pub fn parse_atomic_amount(raw: &str) -> Result<TokenAmount, AmountReject> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AmountReject::Missing);
    }
    if let Some(rest) = s.strip_prefix('-') {
        // Distinguish "-5" (a negative price) from "-abc" (not a number at all),
        // because the first is a source bug worth naming and the second is junk.
        return Err(
            if rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
                AmountReject::Negative
            } else {
                AmountReject::NotAnInteger
            },
        );
    }
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return Err(AmountReject::NotAnInteger);
    }
    alloy::primitives::U256::from_str_radix(s, 10)
        .map(TokenAmount::from)
        .map_err(|_| AmountReject::Overflow)
}

// ============================================================================
// CatalogPaymentOption
// ============================================================================

/// One declared way to pay for a catalog resource.
///
/// The first seven fields are the wire shape of [`PaymentRequirementsV2`] and
/// round-trip with it. The rest are **response-only**: they are resolved when a
/// listing is composed, never written to the store, so they cannot go stale
/// against the deployment table or against which schemes this build serves. The
/// same discipline the health and curation overlays already use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPaymentOption {
    /// The scheme the source declared, never a substitute for it.
    pub scheme: CatalogScheme,

    /// Network in CAIP-2 form.
    pub network: Caip2NetworkId,

    /// Token contract address or account.
    pub asset: MixedAddress,

    /// Amount in the asset's atomic units.
    ///
    /// `maxAmountRequired` is accepted as a spelling of this field and nothing
    /// more: it is the x402 v1 name for the same number. It does not make the
    /// price a range and it does not change the scheme. See [`reconcile_amount`]
    /// for what happens when a document carries both.
    pub amount: TokenAmount,

    /// Recipient address.
    pub pay_to: MixedAddress,

    /// Seconds the payment authorization stays valid.
    pub max_timeout_seconds: u64,

    /// Scheme- or application-specific data, preserved verbatim. For `exact`
    /// this is where the EIP-712 domain lives; for other schemes it is where
    /// their parameters live, which is why dropping it made an `upto` or escrow
    /// listing uninterpretable even when the scheme survived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,

    /// Whether **this facilitator** can settle this option, as opposed to merely
    /// listing it. Response-only; absent means not evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settleable: Option<bool>,

    /// Kebab reason `settleable` is false. Response-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,

    /// Symbol of the token at `asset` on `network`, when it is one we have
    /// registered. Response-only; absent means unknown, which is a different
    /// statement from "dollars". A price cannot be rendered without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_symbol: Option<String>,

    /// Decimals of that deployment. Response-only.
    ///
    /// Decimals are a property of a deployment, not of a token: USDC is 6 nearly
    /// everywhere and **18 on BSC**, and Stellar USDC is 7. Absent means unknown
    /// and the amount must be shown in atomic units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_decimals: Option<u8>,
}

impl CatalogPaymentOption {
    /// Build an option from parts, with every response-only field cleared.
    pub fn new(
        scheme: CatalogScheme,
        network: Caip2NetworkId,
        asset: MixedAddress,
        amount: TokenAmount,
        pay_to: MixedAddress,
        max_timeout_seconds: u64,
    ) -> Self {
        Self {
            scheme,
            network,
            asset,
            amount,
            pay_to,
            max_timeout_seconds,
            extra: None,
            settleable: None,
            unsupported_reason: None,
            asset_symbol: None,
            asset_decimals: None,
        }
    }

    /// Attach `extra`, dropping it if it is outside [`sanitize_extra`]'s bounds.
    pub fn with_extra(mut self, extra: Option<serde_json::Value>) -> Self {
        self.extra = sanitize_extra(extra);
        self
    }

    /// Fill the response-only fields. Idempotent, and it always *overwrites*, so
    /// a value that somehow reached the store can never survive a read.
    pub fn annotate(&mut self) {
        let network = Network::from_caip2(&self.network.to_string());
        let (settleable, reason) = settleability(&self.scheme, network);
        self.settleable = Some(settleable);
        self.unsupported_reason = reason.map(str::to_string);

        match network.and_then(|n| known_asset(n, &self.asset)) {
            Some((symbol, decimals)) => {
                self.asset_symbol = Some(symbol.to_string());
                self.asset_decimals = Some(decimals);
            }
            None => {
                self.asset_symbol = None;
                self.asset_decimals = None;
            }
        }
    }

    /// Drop the response-only fields.
    ///
    /// Called on every path into the registry. The wire form accepts these
    /// fields so a client can round-trip a listing we emitted, but a caller must
    /// not be able to *assert* one: `POST /discovery/register` with
    /// `"settleable": true` would otherwise put a claim about this facilitator's
    /// capabilities into the store, made by someone else.
    pub fn strip_response_only(&mut self) {
        self.settleable = None;
        self.unsupported_reason = None;
        self.asset_symbol = None;
        self.asset_decimals = None;
    }
}

/// Wire form of a payment option, as it arrives from a request body, a
/// well-known document, or our own S3 snapshot.
///
/// A hand-written shadow struct rather than `#[derive(Deserialize)]` on
/// [`CatalogPaymentOption`] for two reasons, both of which bit us:
///
/// - `amount` and `maxAmountRequired` have to be *separate fields*. As a
///   `#[serde(alias)]` they become one field, and a document carrying both is a
///   duplicate-field error — which is how the live Coinbase page was being
///   dropped whole.
/// - `network` has to accept an x402 v1 name (`"base"`) as well as CAIP-2, so
///   that a seller registering directly and a feed republishing the same offer
///   land on the same record.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogPaymentOptionWire {
    scheme: CatalogScheme,
    network: String,
    asset: MixedAddress,
    #[serde(default)]
    amount: Option<serde_json::Value>,
    #[serde(default)]
    max_amount_required: Option<serde_json::Value>,
    pay_to: MixedAddress,
    #[serde(default = "default_max_timeout_seconds")]
    max_timeout_seconds: u64,
    #[serde(
        default,
        deserialize_with = "crate::json_depth::deserialize_bounded_extra"
    )]
    extra: Option<serde_json::Value>,
    // Response-only fields are accepted so a listing we emitted can be read back
    // (an SDK round-tripping our own output, say). They are recomputed on the
    // next read, so a stale one cannot survive.
    #[serde(default)]
    settleable: Option<bool>,
    #[serde(default)]
    unsupported_reason: Option<String>,
    #[serde(default)]
    asset_symbol: Option<String>,
    #[serde(default)]
    asset_decimals: Option<u8>,
}

fn default_max_timeout_seconds() -> u64 {
    DEFAULT_MAX_TIMEOUT_SECS
}

impl<'de> Deserialize<'de> for CatalogPaymentOption {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let w = CatalogPaymentOptionWire::deserialize(deserializer)?;
        let network = resolve_catalog_network(&w.network).ok_or_else(|| {
            serde::de::Error::custom(format!("unrecognized network: {:?}", w.network))
        })?;
        let amount = reconcile_amount(w.amount.as_ref(), w.max_amount_required.as_ref())
            .map_err(|e| serde::de::Error::custom(format!("amount: {e}")))?;
        Ok(CatalogPaymentOption {
            scheme: w.scheme,
            network,
            asset: w.asset,
            amount,
            pay_to: w.pay_to,
            max_timeout_seconds: w.max_timeout_seconds,
            extra: w.extra,
            settleable: w.settleable,
            unsupported_reason: w.unsupported_reason,
            asset_symbol: w.asset_symbol,
            asset_decimals: w.asset_decimals,
        })
        .map(|mut o| {
            o.extra = sanitize_extra(o.extra.take());
            o
        })
    }
}

impl From<PaymentRequirementsV2> for CatalogPaymentOption {
    fn from(r: PaymentRequirementsV2) -> Self {
        Self {
            scheme: CatalogScheme::Known(r.scheme),
            network: r.network,
            asset: r.asset,
            amount: r.amount,
            pay_to: r.pay_to,
            max_timeout_seconds: r.max_timeout_seconds,
            extra: r.extra,
            settleable: None,
            unsupported_reason: None,
            asset_symbol: None,
            asset_decimals: None,
        }
    }
}

/// A catalog option cannot be turned back into payment requirements when its
/// scheme is one this build does not implement.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("payment scheme {0:?} is not implemented by this facilitator")]
pub struct UnsupportedSchemeError(pub String);

impl TryFrom<&CatalogPaymentOption> for PaymentRequirementsV2 {
    type Error = UnsupportedSchemeError;

    fn try_from(o: &CatalogPaymentOption) -> Result<Self, Self::Error> {
        let scheme = o
            .scheme
            .as_known()
            .ok_or_else(|| UnsupportedSchemeError(o.scheme.to_string()))?;
        Ok(PaymentRequirementsV2 {
            scheme,
            network: o.network.clone(),
            asset: o.asset.clone(),
            amount: o.amount,
            pay_to: o.pay_to.clone(),
            max_timeout_seconds: o.max_timeout_seconds,
            extra: o.extra.clone(),
        })
    }
}

// ============================================================================
// Capability: catalogable is not settleable
// ============================================================================

/// Whether this facilitator could settle `scheme` on `network`, and why not.
///
/// Answers a narrower question than "is this a real offer". A `batch-settlement`
/// listing on Base is perfectly real; we simply cannot pay it, and the catalog
/// has to be able to say both things at once.
pub fn settleability(
    scheme: &CatalogScheme,
    network: Option<Network>,
) -> (bool, Option<&'static str>) {
    let Some(known) = scheme.as_known() else {
        return (false, Some("unknown-scheme"));
    };
    let Some(network) = network else {
        return (false, Some("network-not-served"));
    };
    // `upto` needs the Permit2 proxy deployed; the list is maintained by hand
    // against `eth_getCode`, not probed at runtime.
    if known == Scheme::Upto && !crate::upto::types::is_proxy_deployed_on(network) {
        return (false, Some("upto-proxy-not-deployed"));
    }
    (true, None)
}

/// Symbol and decimals of the token deployed at `asset` on `network`.
///
/// Returns `None` when the deployment is not one we have registered, so a caller
/// can print "unknown unit" instead of assuming six decimals and a dollar sign.
pub fn known_asset(network: Network, asset: &MixedAddress) -> Option<(&'static str, u8)> {
    let needle = asset.to_string().to_ascii_lowercase();
    TokenType::all().iter().find_map(|token_type| {
        let deployment = get_token_deployment(network, *token_type)?;
        (deployment.asset.address.to_string().to_ascii_lowercase() == needle)
            .then_some((token_symbol(*token_type), deployment.decimals))
    })
}

/// Display symbol for a token type. Not the serde name (lowercase) and not the
/// EIP-712 domain name (which varies by chain) — the ticker a reader expects.
pub fn token_symbol(t: TokenType) -> &'static str {
    match t {
        TokenType::Usdc => "USDC",
        TokenType::Eurc => "EURC",
        TokenType::Ausd => "AUSD",
        TokenType::Pyusd => "PYUSD",
        TokenType::Usdt => "USDT",
        TokenType::Usdg => "USDG",
        TokenType::Rlusd => "RLUSD",
        TokenType::Xrp => "XRP",
    }
}

// ============================================================================
// Ingestion DTO
// ============================================================================

/// One payment option exactly as an upstream feed published it.
///
/// Every field is optional because a feed is not a contract: this type's job is
/// to survive whatever arrives so [`normalize_declared_option`] can say
/// precisely what was wrong with it, rather than have `serde` fail the whole
/// page over one bad entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredPaymentOption {
    /// Payment scheme.
    pub scheme: Option<String>,
    /// Network, in CAIP-2 form or an x402 v1 name.
    pub network: Option<String>,
    /// Token asset address.
    pub asset: Option<String>,
    /// Amount in atomic units (x402 v2 spelling).
    pub amount: Option<serde_json::Value>,
    /// The x402 v1 spelling of the same number.
    ///
    /// Two fields rather than `#[serde(alias)]`, because an alias makes both
    /// spellings the *same* field and `serde` then rejects a document carrying
    /// both as a duplicate. The live Coinbase CDP feed carries both on 55 of the
    /// 178 payment options on its first page, and at least one such option
    /// appeared on every page sampled on 2026-09-10 — so the alias was failing
    /// the entire page, not one entry.
    pub max_amount_required: Option<serde_json::Value>,
    /// Recipient.
    pub pay_to: Option<String>,
    /// Authorization lifetime.
    pub max_timeout_seconds: Option<u64>,
    /// Scheme-specific parameters. Dropped rather than fatal when out of bounds:
    /// see [`deserialize_tolerant_extra`].
    #[serde(default, deserialize_with = "deserialize_tolerant_extra")]
    pub extra: Option<serde_json::Value>,
}

/// Why a declared option could not be catalogued.
///
/// A rejection is a *loss*, and naming it is the point: the old converter had
/// exactly one outcome for every one of these, and it was a price of zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionReject {
    /// No `scheme` field. We will not assume one.
    SchemeMissing,
    /// A `scheme` that is not a scheme identifier at all.
    SchemeMalformed,
    /// No `network` field.
    NetworkMissing,
    /// A network we cannot name in CAIP-2.
    NetworkUnrecognized,
    /// No `asset` field.
    AssetMissing,
    /// An asset address in no format we parse.
    AssetMalformed,
    /// No `payTo` field.
    PayToMissing,
    /// A recipient address in no format we parse.
    PayToMalformed,
    /// The amount was refused; see [`AmountReject`].
    Amount(AmountReject),
}

impl OptionReject {
    /// Kebab identifier for logs and metrics. Bounded vocabulary — never source text.
    pub fn rule(&self) -> &'static str {
        match self {
            OptionReject::SchemeMissing => "scheme-missing",
            OptionReject::SchemeMalformed => "scheme-malformed",
            OptionReject::NetworkMissing => "network-missing",
            OptionReject::NetworkUnrecognized => "network-unrecognized",
            OptionReject::AssetMissing => "asset-missing",
            OptionReject::AssetMalformed => "asset-malformed",
            OptionReject::PayToMissing => "pay-to-missing",
            OptionReject::PayToMalformed => "pay-to-malformed",
            OptionReject::Amount(a) => a.rule(),
        }
    }
}

impl std::fmt::Display for OptionReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.rule())
    }
}

/// Turn one published payment option into a catalog record, or say why not.
///
/// This is the single normalization point for every ingestion route. A route
/// that skipped it is a route that can preserve what another one erases, which
/// is how the aggregator and `POST /discovery/register` came to disagree about
/// the same JSON.
pub fn normalize_declared_option(
    d: DeclaredPaymentOption,
) -> Result<CatalogPaymentOption, OptionReject> {
    let scheme_raw = d.scheme.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let scheme = match scheme_raw {
        None => return Err(OptionReject::SchemeMissing),
        Some(s) => CatalogScheme::parse(s).ok_or(OptionReject::SchemeMalformed)?,
    };

    let network_raw = d
        .network
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let network = match network_raw {
        None => return Err(OptionReject::NetworkMissing),
        Some(s) => resolve_catalog_network(s).ok_or(OptionReject::NetworkUnrecognized)?,
    };

    let asset_raw = d.asset.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let asset = match asset_raw {
        None => return Err(OptionReject::AssetMissing),
        Some(s) => parse_catalog_address(s).ok_or(OptionReject::AssetMalformed)?,
    };

    let pay_to_raw = d.pay_to.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let pay_to = match pay_to_raw {
        None => return Err(OptionReject::PayToMissing),
        Some(s) => parse_catalog_address(s).ok_or(OptionReject::PayToMalformed)?,
    };

    let amount = reconcile_amount(d.amount.as_ref(), d.max_amount_required.as_ref())
        .map_err(OptionReject::Amount)?;

    Ok(CatalogPaymentOption::new(
        scheme,
        network,
        asset,
        amount,
        pay_to,
        d.max_timeout_seconds.unwrap_or(DEFAULT_MAX_TIMEOUT_SECS),
    )
    .with_extra(d.extra))
}

/// Reconcile the v2 (`amount`) and v1 (`maxAmountRequired`) spellings.
///
/// They are two names for one number. The alias does not make the price a range,
/// and it does not change the scheme: for `exact` the number is the price, for
/// `upto` it is the ceiling, and which of the two it means is decided by
/// `scheme` alone.
///
/// If both are present and they disagree, that is refused rather than resolved.
/// Picking one would be a guess and averaging them would invent a price nobody
/// published. (Measured on the CDP feed, 2026-09-10: of 55 options carrying
/// both, **zero** disagreed — so this branch costs nothing today and is the only
/// safe answer if that ever stops being true.)
pub fn reconcile_amount(
    amount: Option<&serde_json::Value>,
    max_amount_required: Option<&serde_json::Value>,
) -> Result<TokenAmount, AmountReject> {
    let v2 = declared_amount_text(amount);
    let v1 = declared_amount_text(max_amount_required);
    let text = match (v2, v1) {
        (Some(a), Some(b)) => {
            if a.trim() != b.trim() {
                return Err(AmountReject::Conflicting);
            }
            a
        }
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return Err(AmountReject::Missing),
    };
    parse_atomic_amount(&text)
}

/// Recover the literal text a feed used for an amount.
///
/// Feeds spell it as a JSON string; a few spell it as a JSON number. `Number`'s
/// own `to_string` is used rather than `as_u64`, so a float arrives here as
/// `"0.002"` and is rejected as the decimal-in-an-atomic-field that it is,
/// instead of being silently truncated.
fn declared_amount_text(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Resolve a published network identifier to CAIP-2.
///
/// Order matters: CAIP-2 as written wins, then the x402 v1 names this
/// facilitator serves (all of them, from [`Network`]'s own table rather than a
/// second hand-maintained list), then the legacy aliases upstream feeds publish,
/// then a bare EVM chain id.
pub fn resolve_catalog_network(raw: &str) -> Option<Caip2NetworkId> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(id) = Caip2NetworkId::parse(s) {
        return Some(id);
    }
    let lower = s.to_ascii_lowercase();
    // Canonical v1 names. Going through Network keeps this in step with every
    // chain we add instead of drifting from a copy.
    if let Ok(network) = Network::from_str(&lower) {
        if let Ok(id) = Caip2NetworkId::parse(&network.to_caip2()) {
            return Some(id);
        }
    }
    // Aliases other bazaars publish that are not our canonical spellings.
    let aliased = match lower.as_str() {
        "base-mainnet" => Some("base"),
        "mainnet" | "ethereum-mainnet" => Some("ethereum"),
        "sepolia" => Some("ethereum-sepolia"),
        "polygon-mainnet" | "matic" => Some("polygon"),
        "amoy" => Some("polygon-amoy"),
        "optimism-mainnet" => Some("optimism"),
        "arbitrum-mainnet" | "arbitrum-one" => Some("arbitrum"),
        "avalanche-mainnet" | "avalanche-c-chain" => Some("avalanche"),
        "fuji" => Some("avalanche-fuji"),
        "celo-mainnet" => Some("celo"),
        // Celo's testnet moved from Alfajores to Sepolia; the old name still
        // reaches us from feeds that have not caught up.
        "celo-alfajores" | "alfajores" => Some("celo-sepolia"),
        _ => None,
    };
    if let Some(name) = aliased {
        if let Ok(network) = Network::from_str(name) {
            if let Ok(id) = Caip2NetworkId::parse(&network.to_caip2()) {
                return Some(id);
            }
        }
    }
    // A bare EVM chain id. Kept last so it can never shadow a name.
    if let Ok(chain_id) = lower.parse::<u64>() {
        return Some(Caip2NetworkId::eip155(chain_id));
    }
    None
}

/// Parse an address published by a third-party feed.
///
/// Uses the same rules as every other ingestion route (`MixedAddress`'s own
/// deserializer) rather than a second, EVM-only parser — that second parser is
/// why an option on Solana, NEAR, Stellar or Sui was dropped from an aggregated
/// feed while the identical option registered directly was kept.
///
/// One deliberate narrowing: `MixedAddress::Offchain` is refused. Its pattern
/// (`^[A-Za-z0-9][A-Za-z0-9-]{0,34}[A-Za-z0-9]$`) matches ordinary words, so
/// accepting it from an untrusted feed would let any string through as an
/// address. A caller that genuinely needs an off-chain recipient registers it
/// directly.
pub fn parse_catalog_address(raw: &str) -> Option<MixedAddress> {
    let parsed: MixedAddress =
        serde_json::from_value(serde_json::Value::String(raw.trim().to_string())).ok()?;
    match parsed {
        MixedAddress::Offchain(_) => None,
        other => Some(other),
    }
}
