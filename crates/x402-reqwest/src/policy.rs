//! What this buyer is allowed to sign, decided before it signs.
//!
//! # The rule this module exists to enforce
//!
//! A catalog listing is a claim somebody else made about their own price. The
//! `402` that comes back from the actual request is the offer. They can differ,
//! legitimately: a seller may have repriced, and the listing may be a copy of a
//! copy. So the buying decision cannot be made against the listing. It has to be made
//! against **the offer in hand**, every time, before anything is signed.
//!
//! Two things follow, and both are load-bearing:
//!
//! * **A divergence from the listing is not, by itself, a refusal.** If the
//!   offer costs more than the catalog said but still sits inside a policy the
//!   operator already authorised, the payment proceeds. Stopping to ask would
//!   turn every ordinary reprice into a halt, and an agent that halts on
//!   ordinary commerce is an agent nobody can leave running.
//! * **A policy is never widened to fit an offer.** Not by a byte, not once, not
//!   "because the seller says so". If the offer exceeds what was authorised the
//!   answer is a refusal with a concrete cause, and the caller decides whether
//!   to authorise more. There is deliberately no method on this type that raises
//!   a limit from inside an evaluation.
//!
//! # Order of evaluation
//!
//! Fixed, and part of the contract, because the FIRST failing check is the one
//! reported and a caller branches on it:
//!
//! 1. Was the offer readable at all?
//! 2. Has it expired?
//! 3. Is the recipient one we are willing to pay?
//! 4. Does it exceed the per-payment limit for its asset?
//! 5. Does it exceed what remains of the cumulative limit for its asset?
//!
//! Divergence from an advertised quote is recorded at every step and refuses
//! nothing on its own.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use alloy::primitives::U256;
use x402_rs::types::{PaymentRequirements, TokenAmount, TokenAsset, UnreadableOffer};

/// Why a payment was not signed.
///
/// Every variant names something the caller can act on. There is no
/// `PolicyRefusal::Other`: a refusal a caller cannot interpret is a refusal it
/// will paper over.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyRefusal {
    /// The challenge carried offers, and this build could not read any of them.
    ///
    /// Names the schemes the seller actually offered, so the caller can tell
    /// "the seller is broken" from "the seller wants a scheme we do not
    /// implement" -- and so discovering the service still works even though
    /// buying it automatically does not.
    #[error("no offer in this challenge is one this build can pay; offered: {offered:?}")]
    NoReadableOffer { offered: Vec<String> },

    /// The offer's validity window has passed.
    #[error("this offer expired at {valid_until} (now {now}); ask the seller for new terms")]
    OfferExpired { valid_until: u64, now: u64 },

    /// The recipient is not one this policy will pay.
    #[error("this policy does not pay {pay_to}")]
    RecipientNotPermitted { pay_to: String },

    /// The offer is priced in an asset this policy was never given a ceiling
    /// for. Refused rather than permitted: see [`PurchasePolicy::new`].
    #[error("this policy has no budget for {asset}; it pays only what it was told it may pay")]
    AssetNotBudgeted { asset: TokenAsset },

    /// One payment exceeds the per-payment ceiling for its asset.
    #[error("offer of {requested} exceeds the per-payment limit of {allowed} for {asset}")]
    PerPaymentLimit {
        requested: TokenAmount,
        allowed: TokenAmount,
        asset: TokenAsset,
    },

    /// The payment would take total spend past the cumulative ceiling.
    #[error(
        "offer of {requested} would take spend to {would_total}, past the cumulative limit of \
         {allowed} for {asset} (already spent {spent})"
    )]
    CumulativeLimit {
        requested: TokenAmount,
        spent: TokenAmount,
        would_total: TokenAmount,
        allowed: TokenAmount,
        asset: TokenAsset,
    },
}

impl PolicyRefusal {
    /// Stable kebab identifier. Bounded vocabulary, for logs, metrics, and for
    /// an SDK in another language to branch on without parsing English.
    pub fn code(&self) -> &'static str {
        match self {
            PolicyRefusal::NoReadableOffer { .. } => "no-readable-offer",
            PolicyRefusal::OfferExpired { .. } => "offer-expired",
            PolicyRefusal::RecipientNotPermitted { .. } => "recipient-not-permitted",
            PolicyRefusal::AssetNotBudgeted { .. } => "asset-not-budgeted",
            PolicyRefusal::PerPaymentLimit { .. } => "per-payment-limit",
            PolicyRefusal::CumulativeLimit { .. } => "cumulative-limit",
        }
    }
}

/// What a catalog listing advertised, for comparison against the real offer.
///
/// Optional throughout. A buyer that never read a listing simply evaluates the
/// offer against its policy, which is the same decision with one fewer input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedQuote {
    pub asset: TokenAsset,
    pub amount: TokenAmount,
}

/// How the offer in hand compares to what the catalog advertised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteComparison {
    /// No listing was supplied to compare against.
    NotCompared,
    /// Same asset, same amount.
    Matches,
    /// Same asset, different amount. Not a refusal: see the module docs.
    AmountDiffers {
        advertised: TokenAmount,
        offered: TokenAmount,
    },
    /// A different asset entirely. The same number in another currency is not
    /// the same price, so there is nothing to compare.
    DifferentAsset {
        advertised: TokenAsset,
        offered: TokenAsset,
    },
}

impl QuoteComparison {
    pub fn code(&self) -> &'static str {
        match self {
            QuoteComparison::NotCompared => "not-compared",
            QuoteComparison::Matches => "matches",
            QuoteComparison::AmountDiffers { .. } => "amount-differs",
            QuoteComparison::DifferentAsset { .. } => "different-asset",
        }
    }
}

/// A payment this policy permits, and what it noticed on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyApproval {
    pub asset: TokenAsset,
    pub amount: TokenAmount,
    /// How the offer compared to the listing, when there was one. Carried so a
    /// caller can log or surface it; it never changed the decision.
    pub versus_quote: QuoteComparison,
}

/// The spending rules a caller authorised in advance.
///
/// Cloning shares the running total: two clones of one policy spend from the
/// same purse, which is what makes a cumulative limit mean anything when a
/// client is cloned per request.
#[derive(Debug, Clone, Default)]
pub struct PurchasePolicy {
    per_payment: HashMap<TokenAsset, TokenAmount>,
    cumulative: HashMap<TokenAsset, TokenAmount>,
    spent: Arc<Mutex<HashMap<TokenAsset, TokenAmount>>>,
    recipients: Option<HashSet<String>>,
    /// Whether an asset with no configured ceiling may be paid at all.
    allow_unlisted_assets: bool,
}

impl PurchasePolicy {
    /// A policy that pays nothing until it is told what it may pay.
    ///
    /// # Why the default is deny
    ///
    /// The limits are a map keyed by asset, and a map answers "no entry" for
    /// every asset nobody thought of. Permitting on a missing entry means a
    /// budget in USDC is **no budget at all** for any other token: a seller
    /// offering the same resource priced in something unlisted walks straight
    /// past the ceiling, and the wallet will sign it, because the EVM signer
    /// takes the EIP-712 domain from the seller's own `extra` and will happily
    /// sign for a token and a network it has never heard of.
    ///
    /// So an asset with no stated ceiling is refused. A caller that genuinely
    /// wants to pay anything says so, once, with
    /// [`PurchasePolicy::allow_unlisted_assets`], and that sentence is then in
    /// their code where a reader can find it.
    pub fn new() -> Self {
        Self::default()
    }

    /// A policy that permits an asset it was never told about.
    ///
    /// This is what [`crate::X402Payments`] holds when the caller never
    /// supplied a policy, and it exists for exactly one reason: this crate had
    /// no budget before P3, and turning one on silently would refuse payments
    /// that callers are making today. Named rather than defaulted, so choosing
    /// it is visible.
    pub fn permissive() -> Self {
        Self {
            allow_unlisted_assets: true,
            ..Self::default()
        }
    }

    /// Permit assets with no configured ceiling.
    pub fn allow_unlisted_assets(mut self) -> Self {
        self.allow_unlisted_assets = true;
        self
    }

    /// Refuse assets with no configured ceiling. The default; here so the
    /// intent can be written down at a call site that wants it explicit.
    pub fn deny_unlisted_assets(mut self) -> Self {
        self.allow_unlisted_assets = false;
        self
    }

    /// Most this policy will pay in one payment of `asset`.
    pub fn per_payment(mut self, asset: TokenAsset, amount: TokenAmount) -> Self {
        self.per_payment.insert(asset, amount);
        self
    }

    /// Most this policy will pay in `asset` in total, across every payment it
    /// approves for as long as it lives.
    pub fn cumulative(mut self, asset: TokenAsset, amount: TokenAmount) -> Self {
        self.cumulative.insert(asset, amount);
        self
    }

    /// Restrict payment to a set of recipients.
    ///
    /// Addresses are canonicalised **by family**, not by lowercasing the string.
    /// See [`canonical_recipient`]: EVM hex is case-insensitive, and base58 --
    /// Solana, XRPL -- is not. Lowercasing a base58 address produces a string
    /// that is not an address at all, so an allowlist written in the seller's
    /// own spelling would silently never match and every payment to it would be
    /// refused.
    pub fn only_pay(mut self, recipients: impl IntoIterator<Item = String>) -> Self {
        self.recipients = Some(
            recipients
                .into_iter()
                .map(|r| canonical_recipient(&r))
                .collect(),
        );
        self
    }

    /// Total recorded so far for `asset`.
    pub fn spent(&self, asset: &TokenAsset) -> TokenAmount {
        // A poisoned lock means a previous holder panicked while holding it.
        // Reporting zero spent would silently restore a caller's whole budget,
        // so this reports the ceiling instead: the safe direction for money is
        // to refuse, never to permit.
        match self.spent.lock() {
            Ok(spent) => spent.get(asset).copied().unwrap_or(TokenAmount(U256::ZERO)),
            Err(_) => self
                .cumulative
                .get(asset)
                .copied()
                .unwrap_or(TokenAmount(U256::ZERO)),
        }
    }

    /// Decide whether this offer may be signed.
    ///
    /// `now` is passed rather than read so the decision is testable at an exact
    /// instant; money decisions that depend on a hidden clock cannot be pinned.
    ///
    /// **Approving does not record the spend.** Signing can still fail, the
    /// settlement can still be refused, and a cumulative limit that counted
    /// attempts rather than payments would lock a caller out of money it never
    /// spent. Call [`PurchasePolicy::record_spend`] when a payment settles.
    pub fn evaluate(
        &self,
        offer: &PaymentRequirements,
        quote: Option<&AdvertisedQuote>,
        valid_until: Option<u64>,
        now: u64,
    ) -> Result<PolicyApproval, PolicyRefusal> {
        // 2. Expiry. Before anything about money: terms that have lapsed are not
        //    terms, whatever they say.
        if let Some(valid_until) = valid_until {
            if now > valid_until {
                return Err(PolicyRefusal::OfferExpired { valid_until, now });
            }
        }

        // 3. Recipient.
        if let Some(allowed) = &self.recipients {
            let pay_to = canonical_recipient(&offer.pay_to.to_string());
            if !allowed.contains(&pay_to) {
                return Err(PolicyRefusal::RecipientNotPermitted {
                    pay_to: offer.pay_to.to_string(),
                });
            }
        }

        let asset = offer.token_asset();
        let requested = offer.max_amount_required;

        // 4. An asset nobody budgeted for. Checked BEFORE the ceilings, because
        //    the ceilings are a map and a map has no opinion about a key it does
        //    not hold -- which is precisely how an unlisted token would sail
        //    past a budget that looks complete.
        if !self.allow_unlisted_assets
            && !self.per_payment.contains_key(&asset)
            && !self.cumulative.contains_key(&asset)
        {
            return Err(PolicyRefusal::AssetNotBudgeted { asset });
        }

        // 5. Per-payment ceiling.
        if let Some(allowed) = self.per_payment.get(&asset) {
            if requested > *allowed {
                return Err(PolicyRefusal::PerPaymentLimit {
                    requested,
                    allowed: *allowed,
                    asset,
                });
            }
        }

        // 6. Cumulative ceiling.
        if let Some(allowed) = self.cumulative.get(&asset) {
            let spent = self.spent(&asset);
            // Overflow is treated as exceeding the limit. A total we cannot
            // represent is not a total we may spend.
            let would_total = spent
                .checked_add(requested)
                .unwrap_or(TokenAmount(U256::MAX));
            if would_total > *allowed {
                return Err(PolicyRefusal::CumulativeLimit {
                    requested,
                    spent,
                    would_total,
                    allowed: *allowed,
                    asset,
                });
            }
        }

        // The comparison against the listing is made LAST and refuses nothing.
        // It is evidence for the caller, not a gate: a seller repricing inside a
        // policy the operator already authorised is ordinary commerce.
        let versus_quote = match quote {
            None => QuoteComparison::NotCompared,
            Some(q) if q.asset != asset => QuoteComparison::DifferentAsset {
                advertised: q.asset.clone(),
                offered: asset.clone(),
            },
            Some(q) if q.amount != requested => QuoteComparison::AmountDiffers {
                advertised: q.amount,
                offered: requested,
            },
            Some(_) => QuoteComparison::Matches,
        };

        Ok(PolicyApproval {
            asset,
            amount: requested,
            versus_quote,
        })
    }

    /// Record that a payment actually happened.
    ///
    /// Separate from [`PurchasePolicy::evaluate`] on purpose: see its note.
    pub fn record_spend(&self, asset: &TokenAsset, amount: TokenAmount) {
        if let Ok(mut spent) = self.spent.lock() {
            let entry = spent
                .entry(asset.clone())
                .or_insert(TokenAmount(U256::ZERO));
            // Saturating: a total that overflowed would wrap to a small number
            // and hand the caller its budget back.
            *entry = entry.checked_add(amount).unwrap_or(TokenAmount(U256::MAX));
        }
    }
}

pub use x402_rs::types::OFFER_VALIDITY_EXTENSION;

/// Read `validUntil` (Unix seconds) from a challenge's extensions.
///
/// Shape: `extensions["offer-receipt/1"].info.validUntil`. The `{info, schema}`
/// envelope is the one every merged extension uses; reading the number from
/// anywhere else would be reading a field nobody agreed to publish.
///
/// Anything unparseable is `None`, never zero: "the seller said something we
/// could not read" must not become "this offer expired in 1970".
pub fn offer_valid_until(
    extensions: &std::collections::HashMap<String, serde_json::Value>,
) -> Option<u64> {
    extensions
        .get(OFFER_VALIDITY_EXTENSION)?
        .get("info")?
        .get("validUntil")?
        .as_u64()
}

/// Canonical form of a recipient address, for comparison.
///
/// # Why this is not `to_lowercase()`
///
/// It was, and that is only correct for one family. EVM addresses are hex and
/// arrive both checksummed and lowercase, so folding case is right and
/// necessary. **Base58 is case-sensitive** -- Solana and XRPL addresses use both
/// cases as distinct symbols -- so lowercasing one does not produce the same
/// address in a different spelling, it produces a string that is not an address.
///
/// An allowlist written in a seller's own spelling would then never match, and
/// every payment to a legitimate Solana payee would be refused with
/// `recipient-not-permitted`. Worse in the other direction: two distinct base58
/// addresses can fold to the same lowercase string, so an allowlist could admit
/// an address nobody put on it.
///
/// So: hex is folded, everything else is compared exactly.
///
/// # The `0x` is load-bearing
///
/// Only a string that STARTS with `0x` is treated as hex. An allowlist entry
/// written as bare hex (`e4dc96...`) is therefore compared exactly, while the
/// `payTo` on an offer always arrives `0x`-prefixed and is folded -- so the two
/// never match and the payment is refused with `recipient-not-permitted`.
///
/// That fails in the safe direction, and it is still a trap, so it is written
/// down here and in the SDK contract rather than silently normalised: adding a
/// prefix to somebody's allowlist entry is guessing which family they meant, and
/// a bare 32-character base58 string is not distinguishable from bare hex by
/// looking at it.
pub fn canonical_recipient(address: &str) -> String {
    let trimmed = address.trim();
    let is_hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false);
    if is_hex {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// Turn a challenge with nothing payable in it into a refusal that says so.
///
/// Called when selection found no candidate. The point is the message: a caller
/// that learns the seller offered `batch-settlement` knows to look for a
/// facilitator that implements it, where "could not parse the response" would
/// have sent it looking for a bug.
pub fn no_readable_offer(unreadable: &[UnreadableOffer]) -> PolicyRefusal {
    PolicyRefusal::NoReadableOffer {
        offered: unreadable.iter().filter_map(|o| o.scheme.clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x402_rs::network::Network;
    use x402_rs::types::{EvmAddress, MixedAddress, Scheme, X402Version};

    const USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const PAYEE: &str = "0xe4dc963c56979E0260fc146b87eE24F18220e545";
    const OTHER_PAYEE: &str = "0x000000000000000000000000000000000000dEaD";

    fn addr(hex: &str) -> MixedAddress {
        MixedAddress::Evm(hex.parse::<EvmAddress>().unwrap())
    }

    fn asset() -> TokenAsset {
        TokenAsset {
            address: addr(USDC),
            network: Network::Base,
        }
    }

    fn offer_of(amount: u64, pay_to: &str) -> PaymentRequirements {
        PaymentRequirements {
            scheme: Scheme::Exact,
            network: Network::Base,
            max_amount_required: TokenAmount::from(amount),
            resource: "https://api.example.com/thing".parse().unwrap(),
            description: String::new(),
            mime_type: String::new(),
            output_schema: None,
            pay_to: addr(pay_to),
            max_timeout_seconds: 300,
            asset: addr(USDC),
            extra: None,
        }
    }

    fn amount(n: u64) -> TokenAmount {
        TokenAmount::from(n)
    }

    // ========================================================================
    // A divergence from the listing is not a refusal
    // ========================================================================

    #[test]
    fn a_seller_repricing_inside_policy_is_paid_without_asking_anyone() {
        // THE rule. The catalog said 0.01, the offer is 0.03, and the operator
        // authorised up to 0.10. An agent that stops here to ask a human is an
        // agent nobody can leave running, and there is no human to ask.
        let policy = PurchasePolicy::new().per_payment(asset(), amount(100_000));
        let quote = AdvertisedQuote {
            asset: asset(),
            amount: amount(10_000),
        };
        let approval = policy
            .evaluate(&offer_of(30_000, PAYEE), Some(&quote), None, 1_000)
            .expect("inside an authorised policy, this is ordinary commerce");
        assert_eq!(approval.amount, amount(30_000));
        assert_eq!(
            approval.versus_quote,
            QuoteComparison::AmountDiffers {
                advertised: amount(10_000),
                offered: amount(30_000)
            },
            "and the divergence is reported, not hidden"
        );
    }

    #[test]
    fn a_reprice_beyond_policy_is_refused_and_the_policy_is_not_widened() {
        let policy = PurchasePolicy::new().per_payment(asset(), amount(20_000));
        let refusal = policy
            .evaluate(&offer_of(30_000, PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "per-payment-limit");

        // And the limit is exactly where it was. Nothing in an evaluation may
        // raise it: the whole point of authorising in advance is that the thing
        // being evaluated does not get a vote.
        let again = policy
            .evaluate(&offer_of(30_000, PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(again.code(), "per-payment-limit");
        assert!(policy
            .evaluate(&offer_of(20_000, PAYEE), None, None, 1_000)
            .is_ok());
    }

    #[test]
    fn the_same_amount_in_another_asset_is_not_the_same_price() {
        let policy = PurchasePolicy::permissive();
        let other = TokenAsset {
            address: addr(OTHER_PAYEE),
            network: Network::Base,
        };
        let quote = AdvertisedQuote {
            asset: other.clone(),
            amount: amount(30_000),
        };
        let approval = policy
            .evaluate(&offer_of(30_000, PAYEE), Some(&quote), None, 1_000)
            .unwrap();
        assert!(matches!(
            approval.versus_quote,
            QuoteComparison::DifferentAsset { .. }
        ));
    }

    // ========================================================================
    // Expiry
    // ========================================================================

    // ========================================================================
    // An asset nobody budgeted for
    // ========================================================================

    #[test]
    fn an_asset_with_no_ceiling_is_refused_by_default() {
        // The limits are a MAP, and a map has no opinion about a key it does not
        // hold. A budget in USDC that permits an unlisted token is not a budget:
        // the same resource priced in something else walks straight past it, and
        // the EVM signer will sign for a token and a network it has never heard
        // of because the EIP-712 domain comes from the seller's own `extra`.
        let policy = PurchasePolicy::new().per_payment(asset(), amount(100_000));
        let other = TokenAsset {
            address: addr(OTHER_PAYEE),
            network: Network::Base,
        };
        let mut offer = offer_of(1, PAYEE);
        offer.asset = other.address.clone();

        let refusal = policy.evaluate(&offer, None, None, 1_000).unwrap_err();
        assert_eq!(refusal.code(), "asset-not-budgeted");
    }

    #[test]
    fn the_budgeted_asset_still_goes_through() {
        let policy = PurchasePolicy::new().per_payment(asset(), amount(100_000));
        assert!(policy
            .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
            .is_ok());
    }

    #[test]
    fn a_cumulative_ceiling_alone_is_a_budget_for_that_asset() {
        // Either kind of ceiling counts as "this asset was thought about".
        let policy = PurchasePolicy::new().cumulative(asset(), amount(100_000));
        assert!(policy
            .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
            .is_ok());
    }

    #[test]
    fn permitting_unlisted_assets_is_available_and_has_to_be_asked_for() {
        let policy = PurchasePolicy::new().allow_unlisted_assets();
        let other = TokenAsset {
            address: addr(OTHER_PAYEE),
            network: Network::Base,
        };
        let mut offer = offer_of(999_999, PAYEE);
        offer.asset = other.address;
        assert!(policy.evaluate(&offer, None, None, 1_000).is_ok());
    }

    #[test]
    fn the_permissive_constructor_is_the_one_the_middleware_defaults_to() {
        // Backward compatibility, named rather than defaulted, so choosing it is
        // visible in the code that chooses it.
        let mut offer = offer_of(999_999, PAYEE);
        offer.asset = addr(OTHER_PAYEE);
        assert!(PurchasePolicy::permissive()
            .evaluate(&offer, None, None, 1_000)
            .is_ok());
        assert!(PurchasePolicy::new()
            .evaluate(&offer, None, None, 1_000)
            .is_err());
    }

    #[test]
    fn the_asset_check_runs_before_the_ceilings() {
        // Order is contract. A caller branching on the cause must be told to
        // budget the asset, not to raise a ceiling that does not exist.
        let policy = PurchasePolicy::new();
        let refusal = policy
            .evaluate(&offer_of(u64::MAX, PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "asset-not-budgeted");
    }

    // ========================================================================
    // Recipient canonicalisation, by family
    // ========================================================================

    #[test]
    fn a_base58_recipient_is_compared_exactly() {
        // Solana and XRPL addresses are base58, where case is a symbol and not a
        // spelling. Lowercasing one does not produce the same address written
        // differently; it produces a string that is not an address, so an
        // allowlist in the seller's own spelling would never match and every
        // legitimate payment to it would be refused.
        let solana = "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq";
        assert_eq!(canonical_recipient(solana), solana);
        assert_ne!(canonical_recipient(solana), solana.to_lowercase());

        let xrpl = "rfADKkVXBNqK3z72tVSS3LVzAR3psYkonp";
        assert_eq!(canonical_recipient(xrpl), xrpl);
    }

    #[test]
    fn an_evm_recipient_is_still_case_folded() {
        let checksummed = PAYEE;
        assert_eq!(
            canonical_recipient(checksummed),
            checksummed.to_ascii_lowercase()
        );
        assert_eq!(
            canonical_recipient(checksummed),
            canonical_recipient(&checksummed.to_lowercase()),
            "checksummed and lowercase are the same payee"
        );
    }

    #[test]
    fn a_bare_hex_allowlist_entry_is_compared_exactly_and_therefore_will_not_match() {
        // The trap, pinned rather than papered over. A caller who writes an EVM
        // address without `0x` gets a refusal, not a silent match, and the
        // contract says so. Normalising for them would mean guessing the family
        // from a string that does not say which one it is.
        let bare = &PAYEE[2..];
        assert_eq!(canonical_recipient(bare), bare, "compared exactly");
        assert_ne!(
            canonical_recipient(bare),
            canonical_recipient(PAYEE),
            "so an allowlist written this way refuses the payee it meant to allow"
        );

        let policy = PurchasePolicy::permissive().only_pay([bare.to_string()]);
        let refusal = policy
            .evaluate(&offer_of(1, PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "recipient-not-permitted");
    }

    #[test]
    fn two_base58_addresses_that_fold_alike_stay_distinct() {
        // The dangerous direction: folding case could make an allowlist admit an
        // address nobody put on it.
        let a = "SoLaNa1111111111111111111111111111111111111";
        let b = "solana1111111111111111111111111111111111111";
        assert_ne!(canonical_recipient(a), canonical_recipient(b));
    }

    #[test]
    fn an_expired_offer_is_never_signed() {
        let policy = PurchasePolicy::new().per_payment(asset(), amount(100_000));
        let refusal = policy
            .evaluate(&offer_of(10_000, PAYEE), None, Some(1_000), 1_001)
            .unwrap_err();
        assert_eq!(refusal.code(), "offer-expired");
    }

    #[test]
    fn an_offer_expiring_this_second_is_still_valid() {
        // The boundary belongs to the seller: `validUntil` is the last instant
        // the offer stands, not the first instant it does not.
        let policy = PurchasePolicy::permissive();
        assert!(policy
            .evaluate(&offer_of(10_000, PAYEE), None, Some(1_000), 1_000)
            .is_ok());
    }

    #[test]
    fn expiry_is_checked_before_the_money() {
        // The order is part of the contract: a caller branching on the cause
        // must get "ask for new terms", not "raise your budget", for an offer
        // that lapsed and also happened to be expensive.
        let policy = PurchasePolicy::new().per_payment(asset(), amount(1));
        let refusal = policy
            .evaluate(&offer_of(999_999, PAYEE), None, Some(1_000), 2_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "offer-expired");
    }

    // ========================================================================
    // Cumulative budget
    // ========================================================================

    #[test]
    fn a_cumulative_limit_counts_what_was_actually_spent() {
        let policy = PurchasePolicy::new().cumulative(asset(), amount(25_000));
        // Three payments of 10 000: the first two fit, the third does not.
        for _ in 0..2 {
            let approval = policy
                .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
                .expect("inside the cumulative limit");
            policy.record_spend(&approval.asset, approval.amount);
        }
        let refusal = policy
            .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "cumulative-limit");
        assert_eq!(policy.spent(&asset()), amount(20_000));
    }

    #[test]
    fn evaluating_does_not_spend() {
        // Signing can fail and a settlement can be refused. A limit that counted
        // attempts would lock a caller out of money it never spent.
        let policy = PurchasePolicy::new().cumulative(asset(), amount(25_000));
        for _ in 0..10 {
            policy
                .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
                .expect("evaluation is not a payment");
        }
        assert_eq!(policy.spent(&asset()), amount(0));
    }

    #[test]
    fn a_clone_spends_from_the_same_purse() {
        // A middleware is cloned per request. If each clone had its own total,
        // a cumulative limit would mean nothing at all.
        let policy = PurchasePolicy::new().cumulative(asset(), amount(15_000));
        let clone = policy.clone();
        let approval = policy
            .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
            .unwrap();
        policy.record_spend(&approval.asset, approval.amount);

        let refusal = clone
            .evaluate(&offer_of(10_000, PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "cumulative-limit");
    }

    // ========================================================================
    // Recipient
    // ========================================================================

    #[test]
    fn a_recipient_outside_the_list_is_refused_however_cheap() {
        // Permissive on assets, so the cause under test is the recipient and
        // not a missing budget: the recipient check runs first either way.
        let policy = PurchasePolicy::permissive().only_pay([PAYEE.to_string()]);
        let refusal = policy
            .evaluate(&offer_of(1, OTHER_PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "recipient-not-permitted");
    }

    #[test]
    fn a_recipients_case_is_not_a_different_recipient() {
        // EVM addresses arrive checksummed and lowercase from different sellers.
        let policy = PurchasePolicy::permissive().only_pay([PAYEE.to_lowercase()]);
        assert!(policy
            .evaluate(&offer_of(1, PAYEE), None, None, 1_000)
            .is_ok());
    }

    // ========================================================================
    // Unreadable offers
    // ========================================================================

    #[test]
    fn a_challenge_we_cannot_pay_names_what_was_offered() {
        let refusal = no_readable_offer(&[
            UnreadableOffer {
                scheme: Some("batch-settlement".into()),
            },
            UnreadableOffer {
                scheme: Some("agent-pay".into()),
            },
        ]);
        assert_eq!(refusal.code(), "no-readable-offer");
        let message = refusal.to_string();
        assert!(message.contains("batch-settlement"), "{message}");
        assert!(
            message.contains("agent-pay"),
            "a caller learns what the seller wanted, not that we failed to parse"
        );
    }

    #[test]
    fn every_refusal_has_a_bounded_code() {
        // An SDK in another language branches on these. They are a vocabulary,
        // not prose, and this test is what stops one drifting into a sentence.
        for (refusal, code) in [
            (no_readable_offer(&[]), "no-readable-offer"),
            (
                PolicyRefusal::OfferExpired {
                    valid_until: 1,
                    now: 2,
                },
                "offer-expired",
            ),
            (
                PolicyRefusal::RecipientNotPermitted {
                    pay_to: PAYEE.into(),
                },
                "recipient-not-permitted",
            ),
            (
                PolicyRefusal::PerPaymentLimit {
                    requested: amount(2),
                    allowed: amount(1),
                    asset: asset(),
                },
                "per-payment-limit",
            ),
            (
                PolicyRefusal::CumulativeLimit {
                    requested: amount(2),
                    spent: amount(1),
                    would_total: amount(3),
                    allowed: amount(2),
                    asset: asset(),
                },
                "cumulative-limit",
            ),
        ] {
            assert_eq!(refusal.code(), code);
            assert!(!refusal.to_string().is_empty());
        }
    }

    // ========================================================================
    // The validity extension
    // ========================================================================

    fn extensions_with(
        value: serde_json::Value,
    ) -> std::collections::HashMap<String, serde_json::Value> {
        let mut e = std::collections::HashMap::new();
        e.insert(OFFER_VALIDITY_EXTENSION.to_string(), value);
        e
    }

    #[test]
    fn validity_is_read_from_the_versioned_envelope() {
        let e = extensions_with(serde_json::json!({
            "info": {"validUntil": 1_700_000_000u64},
            "schema": {}
        }));
        assert_eq!(offer_valid_until(&e), Some(1_700_000_000));
    }

    #[test]
    fn a_key_without_a_version_is_not_this_extension() {
        // The transport of offer-and-receipt may still change. A value read from
        // an unversioned key could not be compared against anything later.
        let mut e = std::collections::HashMap::new();
        e.insert(
            "offer-receipt".to_string(),
            serde_json::json!({"info": {"validUntil": 1u64}}),
        );
        assert_eq!(offer_valid_until(&e), None);
    }

    #[test]
    fn an_unreadable_validity_is_no_validity_rather_than_the_epoch() {
        // "The seller said something we could not read" must never become
        // "this offer expired in 1970", which would refuse every payment.
        for bad in [
            serde_json::json!({"info": {"validUntil": "soon"}}),
            serde_json::json!({"info": {}}),
            serde_json::json!({"validUntil": 1_700_000_000u64}),
            serde_json::json!("nope"),
        ] {
            assert_eq!(offer_valid_until(&extensions_with(bad)), None);
        }
    }

    #[test]
    fn no_extension_at_all_means_no_stated_expiry() {
        assert_eq!(offer_valid_until(&std::collections::HashMap::new()), None);
        // ... and an offer with no stated expiry is payable.
        assert!(PurchasePolicy::permissive()
            .evaluate(&offer_of(1, PAYEE), None, None, u64::MAX)
            .is_ok());
    }

    #[test]
    fn a_permissive_policy_pays_and_says_it_did_not_compare() {
        // `permissive()`, not `new()`: since the asset check landed, a policy
        // with no ceilings at all pays nothing. That is the safe default and
        // this test now names which constructor it is exercising.
        let approval = PurchasePolicy::permissive()
            .evaluate(&offer_of(999_999_999, PAYEE), None, None, 1_000)
            .unwrap();
        assert_eq!(approval.versus_quote, QuoteComparison::NotCompared);
        let _ = X402Version::V1;
    }
}
