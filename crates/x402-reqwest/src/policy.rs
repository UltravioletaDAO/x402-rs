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
}

impl PurchasePolicy {
    pub fn new() -> Self {
        Self::default()
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

    /// Restrict payment to a set of recipients. Addresses are compared
    /// case-insensitively, because EVM addresses arrive in both checksummed and
    /// lowercase form and a case difference is not a different payee.
    pub fn only_pay(mut self, recipients: impl IntoIterator<Item = String>) -> Self {
        self.recipients = Some(recipients.into_iter().map(|r| r.to_lowercase()).collect());
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
            let pay_to = offer.pay_to.to_string().to_lowercase();
            if !allowed.contains(&pay_to) {
                return Err(PolicyRefusal::RecipientNotPermitted {
                    pay_to: offer.pay_to.to_string(),
                });
            }
        }

        let asset = offer.token_asset();
        let requested = offer.max_amount_required;

        // 4. Per-payment ceiling.
        if let Some(allowed) = self.per_payment.get(&asset) {
            if requested > *allowed {
                return Err(PolicyRefusal::PerPaymentLimit {
                    requested,
                    allowed: *allowed,
                    asset,
                });
            }
        }

        // 5. Cumulative ceiling.
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
        let policy = PurchasePolicy::new();
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
        let policy = PurchasePolicy::new();
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
        let policy = PurchasePolicy::new().only_pay([PAYEE.to_string()]);
        let refusal = policy
            .evaluate(&offer_of(1, OTHER_PAYEE), None, None, 1_000)
            .unwrap_err();
        assert_eq!(refusal.code(), "recipient-not-permitted");
    }

    #[test]
    fn a_recipients_case_is_not_a_different_recipient() {
        // EVM addresses arrive checksummed and lowercase from different sellers.
        let policy = PurchasePolicy::new().only_pay([PAYEE.to_lowercase()]);
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
        assert!(PurchasePolicy::new()
            .evaluate(&offer_of(1, PAYEE), None, None, u64::MAX)
            .is_ok());
    }

    #[test]
    fn a_policy_with_no_limits_permits_and_says_it_did_not_compare() {
        let approval = PurchasePolicy::new()
            .evaluate(&offer_of(999_999_999, PAYEE), None, None, 1_000)
            .unwrap();
        assert_eq!(approval.versus_quote, QuoteComparison::NotCompared);
        let _ = X402Version::V1;
    }
}
