//! Commercial evidence: what was advertised, offered, authorised, charged and
//! delivered, and **who says so** for each.
//!
//! # Why this is a separate envelope
//!
//! dx402 already proves one thing very well: that the bytes a buyer recovers are
//! the bytes that were delivered. `contentHash` is keccak of the plaintext body,
//! the facilitator signs an [`crate::dx402::types::EvidenceReceipt`] over it, and
//! anyone holding our public key can check that offline.
//!
//! What it cannot answer is commercial. *Was this the price the catalog
//! advertised? Was the charge inside the ceiling the buyer authorised? Which of
//! the seller's options was actually bought?* Those are five different claims
//! made by four different parties, and folding them into the delivery hash would
//! do two unacceptable things at once: it would change what that hash means, and
//! it would put the buyer's private request context inside a value that already
//! circulates.
//!
//! So this is **complementary and separately versioned**. It references the
//! existing evidence by `paymentId` and `contentHash`, and it never alters
//! either. Every existing receipt stays valid; every existing check keeps
//! passing. There is a test for exactly that, and it is the one that matters
//! most in this file.
//!
//! # Nobody speaks for anybody else
//!
//! The single most important property here is that each section records **who
//! asserted it**. A snapshot the buyer hands us is not a statement by the bazaar.
//! An archived HTTPS response is not an offer signed by the seller. A number we
//! read from the chain is not something either party told us.
//!
//! Collapsing those into one undifferentiated blob would produce evidence that
//! looks authoritative and is not: the whole value of keeping it is being able
//! to say, later, which parts somebody can be held to.
//!
//! # A signature is not a certificate of truth
//!
//! When a section carries a signature, that signature proves the signer emitted
//! those bytes. It does not make the contents true, it does not extend to any
//! other section, and it does not make an archived price current. Archiving a
//! price does not keep it valid. Nothing in this module claims otherwise, and
//! the wording of [`Assertion`] is deliberate about it.

use serde::{Deserialize, Serialize};

use crate::types::{MixedAddress, TokenAmount};

/// Format version of the commercial envelope.
///
/// Its **own** version, deliberately separate from
/// [`crate::dx402::types::DX402_VERSION`]: this envelope may evolve without
/// touching the delivery evidence, and the delivery evidence must be able to
/// evolve without silently redefining what a commercial claim meant.
pub const COMMERCIAL_EVIDENCE_VERSION: u8 = 1;

/// Who asserted a piece of evidence.
///
/// Not a trust level. A buyer-asserted snapshot is not worth less than a
/// chain-read amount in general -- it is worth something *different*, and a
/// reader has to know which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertor {
    /// The buyer handed us this. It is their record of what they saw.
    ///
    /// In particular a listing snapshot is asserted by the buyer, **never by the
    /// bazaar that published it**: we did not witness the read, and the bazaar
    /// signed nothing.
    Buyer,
    /// The seller emitted these bytes, in a `402` or in a signed offer.
    Seller,
    /// This facilitator observed or computed it.
    Facilitator,
    /// Read from a settled transaction on a public ledger.
    Chain,
}

impl Assertor {
    pub fn as_str(self) -> &'static str {
        match self {
            Assertor::Buyer => "buyer",
            Assertor::Seller => "seller",
            Assertor::Facilitator => "facilitator",
            Assertor::Chain => "chain",
        }
    }
}

/// How strongly a section is backed.
///
/// A ladder about *provenance*, not about truth: `Signed` means the bytes came
/// with a signature this party produced, and nothing more. It does not certify
/// the contents, it does not extend to a neighbouring section, and it does not
/// make an archived price current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backing {
    /// Somebody told us. Kept because it is what they said.
    Stated,
    /// We archived the bytes as they arrived over an authenticated transport.
    Archived,
    /// The bytes arrived with a signature by the asserting party.
    Signed,
    /// Read from a public ledger, where anyone can check it again.
    OnChain,
}

impl Backing {
    pub fn as_str(self) -> &'static str {
        match self {
            Backing::Stated => "stated",
            Backing::Archived => "archived",
            Backing::Signed => "signed",
            Backing::OnChain => "on_chain",
        }
    }
}

/// One claim, and its provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assertion<T> {
    /// Who says so.
    pub asserted_by: Assertor,
    /// How it is backed. See [`Backing`]: never a statement that the content is
    /// true.
    pub backing: Backing,
    /// When the asserting party observed it (Unix seconds), when that is known.
    ///
    /// Absent means unknown and stays unknown. Filling it with the time we
    /// assembled the envelope would date somebody else's observation with our
    /// clock, which is the mistake the pricing work spent P1 removing from the
    /// catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<u64>,
    /// The claim itself.
    pub value: T,
}

impl<T> Assertion<T> {
    pub fn new(asserted_by: Assertor, backing: Backing, value: T) -> Self {
        Self {
            asserted_by,
            backing,
            observed_at: None,
            value,
        }
    }

    pub fn observed_at(mut self, at: u64) -> Self {
        self.observed_at = Some(at);
        self
    }
}

/// What the catalog advertised, as the buyer read it.
///
/// **Asserted by the buyer.** The bazaar signed nothing and we did not witness
/// the read; this is the buyer's record of what they were shown, which is
/// exactly what makes it useful in a dispute and exactly why it must not be
/// labelled as the bazaar's word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingSnapshot {
    /// Where the listing was read from.
    pub source: String,
    /// The resource the listing described.
    pub resource: String,
    /// Digest of the listing as read, so two snapshots can be compared without
    /// storing either in a public index.
    pub digest: String,
    /// The date the SOURCE claimed for its content, when it claimed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_updated_at: Option<u64>,
    /// The request context the listing was read for, in the vocabulary P1
    /// established (`"GET http anon"`). A listing read without context does not
    /// describe a price for a parameterised purchase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// The offer that was actually accepted.
///
/// `raw` is preserved **verbatim**: the bytes the seller emitted, not a
/// re-serialisation of our parse of them. A re-serialisation cannot be checked
/// against a signature, and a signature over bytes we reconstructed proves
/// nothing about the bytes they sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedOffer {
    /// The offer document as it arrived, base64 of the original bytes.
    pub raw: String,
    /// Which option of the challenge was taken, by its commercial identity --
    /// never by its index in `accepts`, which is not stable across a reprice.
    pub option: OptionIdentity,
    /// The seller's signature over `raw`, when the offer carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// The last instant the seller said these terms stood, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<u64>,
}

/// What identifies one payment option commercially.
///
/// Scheme, network, asset and recipient. Deliberately not the index in
/// `accepts`: an index is a position in a list the seller can reorder, and two
/// offers at the same index on two days are not the same offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionIdentity {
    pub scheme: String,
    pub network: String,
    pub asset: MixedAddress,
    pub pay_to: MixedAddress,
}

/// The ceiling the buyer authorised.
///
/// Kept apart from what was charged, because for `upto` they are different
/// numbers and conflating them is how a catalog ceiling gets rewritten to the
/// last charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Authorization {
    /// Maximum the signed authorisation permits.
    pub max_amount: TokenAmount,
    /// Rate and unit, when the scheme prices by consumption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// What was actually charged, and where to check it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settlement {
    /// The amount that moved.
    pub effective_amount: TokenAmount,
    pub tx_hash: String,
    pub network: String,
    /// Consumption the seller reported, for schemes that meter.
    ///
    /// The buyer's signature over a ceiling does not prove the measurement was
    /// right; this is the seller's claim about it, labelled as such.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_consumption: Option<String>,
}

/// Which parts of the commercial story this envelope actually carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    /// Advertisement, offer, authorisation, settlement and delivery are all
    /// present.
    Complete,
    /// Some sections are missing. They are named in
    /// [`CommercialEvidence::missing`].
    Partial,
}

/// The commercial envelope.
///
/// References the delivery evidence rather than restating it: `payment_id` and
/// `delivery_content_hash` are the join, and neither is recomputed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommercialEvidence {
    /// This envelope's own format version. See [`COMMERCIAL_EVIDENCE_VERSION`].
    pub cv: u8,
    /// The payment this describes, same handle the delivery evidence uses.
    pub payment_id: String,
    /// The delivery hash, **copied and never recomputed**.
    ///
    /// Its meaning is fixed by the existing format: keccak256 of the delivered
    /// plaintext. Nothing in this envelope enters it. That is the invariant this
    /// whole design exists to preserve, and there is a test that fails if a
    /// future change folds commercial data into it.
    pub delivery_content_hash: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<Assertion<ListingSnapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<Assertion<AcceptedOffer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<Assertion<Authorization>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement: Option<Assertion<Settlement>>,
}

/// A section that is not present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingSection {
    Listing,
    Offer,
    Authorization,
    Settlement,
}

impl MissingSection {
    pub fn as_str(self) -> &'static str {
        match self {
            MissingSection::Listing => "listing",
            MissingSection::Offer => "offer",
            MissingSection::Authorization => "authorization",
            MissingSection::Settlement => "settlement",
        }
    }
}

impl CommercialEvidence {
    /// An envelope with nothing in it but the join to the delivery evidence.
    ///
    /// Every section is optional and added as it becomes available. Partial
    /// evidence with an explicit status beats no evidence, and it very much
    /// beats evidence that looks complete because the gaps were filled with
    /// assumptions.
    pub fn new(payment_id: impl Into<String>, delivery_content_hash: impl Into<String>) -> Self {
        Self {
            cv: COMMERCIAL_EVIDENCE_VERSION,
            payment_id: payment_id.into(),
            delivery_content_hash: delivery_content_hash.into(),
            listing: None,
            offer: None,
            authorization: None,
            settlement: None,
        }
    }

    pub fn with_listing(mut self, listing: Assertion<ListingSnapshot>) -> Self {
        self.listing = Some(listing);
        self
    }

    pub fn with_offer(mut self, offer: Assertion<AcceptedOffer>) -> Self {
        self.offer = Some(offer);
        self
    }

    pub fn with_authorization(mut self, authorization: Assertion<Authorization>) -> Self {
        self.authorization = Some(authorization);
        self
    }

    pub fn with_settlement(mut self, settlement: Assertion<Settlement>) -> Self {
        self.settlement = Some(settlement);
        self
    }

    /// Which sections are absent.
    pub fn missing(&self) -> Vec<MissingSection> {
        let mut missing = Vec::new();
        if self.listing.is_none() {
            missing.push(MissingSection::Listing);
        }
        if self.offer.is_none() {
            missing.push(MissingSection::Offer);
        }
        if self.authorization.is_none() {
            missing.push(MissingSection::Authorization);
        }
        if self.settlement.is_none() {
            missing.push(MissingSection::Settlement);
        }
        missing
    }

    /// Whether every section is present.
    ///
    /// Note what this does NOT say: a complete envelope is not a verified one.
    /// It means all five claims are recorded with their provenance, not that
    /// they agree, and not that anybody has checked them against each other.
    pub fn completeness(&self) -> Completeness {
        if self.missing().is_empty() {
            Completeness::Complete
        } else {
            Completeness::Partial
        }
    }

    /// Whether the charge stayed inside the authorised ceiling.
    ///
    /// `None` when either half is missing -- an unanswerable question gets no
    /// answer rather than a reassuring default. For `exact` the two numbers are
    /// normally equal; for `upto` the charge is expected to be lower, and a
    /// charge ABOVE the ceiling is the thing worth finding.
    pub fn charge_within_authorization(&self) -> Option<bool> {
        let max = self.authorization.as_ref()?.value.max_amount;
        let effective = self.settlement.as_ref()?.value.effective_amount;
        Some(effective <= max)
    }

    /// The view safe to put in a public index.
    ///
    /// Keeps the joins and the provenance labels; drops everything that could
    /// carry a private request, a credential or a personalised quote -- which is
    /// the offer bytes, the listing digest's context, and the seller's reported
    /// consumption. Those live only inside the encrypted evidence.
    ///
    /// The rule is simple and worth stating plainly: **an index says that
    /// evidence exists and what shape it has. It never says what is in it.**
    pub fn public_view(&self) -> serde_json::Value {
        serde_json::json!({
            "cv": self.cv,
            "paymentId": self.payment_id,
            "deliveryContentHash": self.delivery_content_hash,
            "completeness": self.completeness(),
            "missing": self.missing().iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            "sections": {
                "listing": self.listing.as_ref().map(|a| section_label(a.asserted_by, a.backing)),
                "offer": self.offer.as_ref().map(|a| section_label(a.asserted_by, a.backing)),
                "authorization": self.authorization.as_ref().map(|a| section_label(a.asserted_by, a.backing)),
                "settlement": self.settlement.as_ref().map(|a| section_label(a.asserted_by, a.backing)),
            },
        })
    }
}

fn section_label(asserted_by: Assertor, backing: Backing) -> serde_json::Value {
    serde_json::json!({ "assertedBy": asserted_by.as_str(), "backing": backing.as_str() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EvmAddress;

    const PAYMENT_ID: &str = "0xabc123";
    const DELIVERY_HASH: &str = "0xdeadbeefcafe";

    fn addr(hex: &str) -> MixedAddress {
        MixedAddress::Evm(hex.parse::<EvmAddress>().unwrap())
    }

    fn option() -> OptionIdentity {
        OptionIdentity {
            scheme: "upto".into(),
            network: "eip155:8453".into(),
            asset: addr("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            pay_to: addr("0xe4dc963c56979E0260fc146b87eE24F18220e545"),
        }
    }

    fn full() -> CommercialEvidence {
        CommercialEvidence::new(PAYMENT_ID, DELIVERY_HASH)
            .with_listing(
                Assertion::new(
                    Assertor::Buyer,
                    Backing::Stated,
                    ListingSnapshot {
                        source: "https://facilitator.example/discovery/resources".into(),
                        resource: "https://api.example.com/thing".into(),
                        digest: "0x1111".into(),
                        source_updated_at: Some(1_700_000_000),
                        context: Some("GET http anon".into()),
                    },
                )
                .observed_at(1_700_000_100),
            )
            .with_offer(Assertion::new(
                Assertor::Seller,
                Backing::Archived,
                AcceptedOffer {
                    raw: "eyJhY2NlcHRzIjpbXX0=".into(),
                    option: option(),
                    signature: None,
                    valid_until: Some(1_700_000_500),
                },
            ))
            .with_authorization(Assertion::new(
                Assertor::Buyer,
                Backing::Signed,
                Authorization {
                    max_amount: TokenAmount::from(100_000u64),
                    unit: Some("per 1k tokens".into()),
                },
            ))
            .with_settlement(Assertion::new(
                Assertor::Chain,
                Backing::OnChain,
                Settlement {
                    effective_amount: TokenAmount::from(30_000u64),
                    tx_hash: "0xfeed".into(),
                    network: "eip155:8453".into(),
                    reported_consumption: Some("3200 tokens".into()),
                },
            ))
    }

    // ========================================================================
    // The invariant the whole design exists to preserve
    // ========================================================================

    #[test]
    fn the_delivery_hash_is_carried_and_never_recomputed() {
        // Folding commercial data into `contentHash` would redefine what every
        // receipt already in the wild attests to, and would put the buyer's
        // request context inside a value that circulates. The envelope COPIES
        // the hash; nothing it holds enters it.
        let evidence = full();
        assert_eq!(evidence.delivery_content_hash, DELIVERY_HASH);

        // Same delivery, wildly different commerce: the hash is the same,
        // because it is a fact about the bytes delivered and nothing else.
        let barely_any = CommercialEvidence::new(PAYMENT_ID, DELIVERY_HASH);
        assert_eq!(
            barely_any.delivery_content_hash,
            evidence.delivery_content_hash
        );
    }

    #[test]
    fn the_envelope_versions_itself_separately_from_the_delivery_evidence() {
        // Two formats that must be free to move independently: a commercial
        // change must not look like a delivery change, and vice versa.
        assert_eq!(full().cv, COMMERCIAL_EVIDENCE_VERSION);
        assert_eq!(COMMERCIAL_EVIDENCE_VERSION, 1);
    }

    // ========================================================================
    // Nobody speaks for anybody else
    // ========================================================================

    #[test]
    fn the_listing_snapshot_is_the_buyers_word_not_the_bazaars() {
        // We did not witness the read and the bazaar signed nothing. Labelling
        // this as the bazaar's statement would manufacture an authority that
        // does not exist.
        let evidence = full();
        let listing = evidence.listing.as_ref().unwrap();
        assert_eq!(listing.asserted_by, Assertor::Buyer);
        assert_ne!(listing.backing, Backing::Signed);
    }

    #[test]
    fn an_archived_response_is_not_a_signed_offer() {
        let evidence = full();
        let offer = evidence.offer.as_ref().unwrap();
        assert_eq!(offer.backing, Backing::Archived);
        assert!(
            offer.value.signature.is_none(),
            "archived means we kept the bytes, not that anybody signed them"
        );
        assert!(Backing::Archived < Backing::Signed);
    }

    #[test]
    fn every_section_says_who_asserted_it() {
        let evidence = full();
        for by in [
            evidence.listing.as_ref().map(|a| a.asserted_by),
            evidence.offer.as_ref().map(|a| a.asserted_by),
            evidence.authorization.as_ref().map(|a| a.asserted_by),
            evidence.settlement.as_ref().map(|a| a.asserted_by),
        ] {
            assert!(by.is_some(), "a section without an assertor is a rumour");
        }
        assert_eq!(
            evidence.settlement.as_ref().unwrap().asserted_by,
            Assertor::Chain
        );
    }

    #[test]
    fn an_unknown_observation_date_stays_unknown() {
        // Dating somebody else's observation with our clock is the mistake P1
        // removed from the catalog. It does not get to come back here.
        let stated = Assertion::new(
            Assertor::Seller,
            Backing::Stated,
            Authorization {
                max_amount: TokenAmount::from(1u64),
                unit: None,
            },
        );
        assert_eq!(stated.observed_at, None);
        let json = serde_json::to_string(&stated).unwrap();
        assert!(
            !json.contains("observedAt"),
            "an absent date must be absent on the wire too: {json}"
        );
    }

    // ========================================================================
    // The five things the acceptance criterion asks to distinguish
    // ========================================================================

    #[test]
    fn advertisement_offer_ceiling_charge_and_delivery_are_all_distinguishable() {
        let e = full();
        assert_eq!(e.listing.as_ref().unwrap().value.digest, "0x1111");
        assert_eq!(e.offer.as_ref().unwrap().value.option.scheme, "upto");
        assert_eq!(
            e.authorization.as_ref().unwrap().value.max_amount,
            TokenAmount::from(100_000u64)
        );
        assert_eq!(
            e.settlement.as_ref().unwrap().value.effective_amount,
            TokenAmount::from(30_000u64)
        );
        assert_eq!(e.delivery_content_hash, DELIVERY_HASH);
        assert_eq!(e.completeness(), Completeness::Complete);
    }

    #[test]
    fn an_upto_charge_below_its_ceiling_is_not_a_discrepancy() {
        // 0.10 authorised, 0.03 charged. Compatible by definition, and the
        // catalog ceiling does not become 0.03.
        let e = full();
        assert_eq!(e.charge_within_authorization(), Some(true));
        assert_eq!(
            e.authorization.as_ref().unwrap().value.max_amount,
            TokenAmount::from(100_000u64),
            "the ceiling is not rewritten to the last charge"
        );
    }

    #[test]
    fn a_charge_above_the_ceiling_is_visible() {
        let mut e = full();
        e.settlement.as_mut().unwrap().value.effective_amount = TokenAmount::from(200_000u64);
        assert_eq!(e.charge_within_authorization(), Some(false));
    }

    #[test]
    fn an_unanswerable_question_gets_no_answer() {
        let e = CommercialEvidence::new(PAYMENT_ID, DELIVERY_HASH);
        assert_eq!(
            e.charge_within_authorization(),
            None,
            "no ceiling and no charge is not 'within'"
        );
    }

    // ========================================================================
    // Partial evidence, explicitly
    // ========================================================================

    #[test]
    fn partial_evidence_names_what_is_missing() {
        // Better than nothing, and much better than something that looks
        // complete because the gaps were filled with assumptions.
        let e = CommercialEvidence::new(PAYMENT_ID, DELIVERY_HASH).with_settlement(Assertion::new(
            Assertor::Chain,
            Backing::OnChain,
            Settlement {
                effective_amount: TokenAmount::from(1u64),
                tx_hash: "0xfeed".into(),
                network: "eip155:8453".into(),
                reported_consumption: None,
            },
        ));
        assert_eq!(e.completeness(), Completeness::Partial);
        let missing: Vec<&str> = e.missing().iter().map(|m| m.as_str()).collect();
        assert_eq!(missing, vec!["listing", "offer", "authorization"]);
    }

    #[test]
    fn complete_does_not_mean_verified() {
        // All five claims recorded with their provenance. Not that they agree,
        // and not that anybody checked them against each other.
        let mut e = full();
        e.settlement.as_mut().unwrap().value.effective_amount = TokenAmount::from(999_999u64);
        assert_eq!(e.completeness(), Completeness::Complete);
        assert_eq!(e.charge_within_authorization(), Some(false));
    }

    // ========================================================================
    // Privacy
    // ========================================================================

    #[test]
    fn the_public_view_carries_no_bodies_no_quotes_and_no_consumption() {
        let e = full();
        let public = serde_json::to_string(&e.public_view()).unwrap();

        for private in [
            "eyJhY2NlcHRzIjpbXX0=", // the offer bytes
            "3200 tokens",          // the seller's reported consumption
            "per 1k tokens",        // the personalised unit
            "GET http anon",        // the buyer's request context
            "0x1111",               // the listing digest
        ] {
            assert!(
                !public.contains(private),
                "a public index must not carry {private:?}: {public}"
            );
        }

        // What it DOES carry: that evidence exists, and what shape it has.
        assert!(public.contains("paymentId"));
        assert!(public.contains("deliveryContentHash"));
        assert!(public.contains("complete"));
        assert!(public.contains("buyer"));
        assert!(public.contains("on_chain"));
    }

    #[test]
    fn the_public_view_of_partial_evidence_says_which_parts_are_absent() {
        let e = CommercialEvidence::new(PAYMENT_ID, DELIVERY_HASH);
        let public = e.public_view();
        assert_eq!(public["completeness"], "partial");
        assert_eq!(public["missing"].as_array().unwrap().len(), 4);
        assert!(public["sections"]["offer"].is_null());
    }

    // ========================================================================
    // Round trip
    // ========================================================================

    #[test]
    fn an_envelope_survives_a_round_trip_with_its_labels_intact() {
        let e = full();
        let back: CommercialEvidence =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        assert_eq!(back.offer.unwrap().value.raw, "eyJhY2NlcHRzIjpbXX0=");
    }

    #[test]
    fn the_offer_bytes_are_preserved_verbatim() {
        // A re-serialisation of our parse cannot be checked against a signature,
        // and a signature over bytes we reconstructed proves nothing about the
        // bytes they sent.
        let raw = "eyJ3ZWlyZCI6ICAgInNwYWNpbmcifQ==";
        let e = CommercialEvidence::new(PAYMENT_ID, DELIVERY_HASH).with_offer(Assertion::new(
            Assertor::Seller,
            Backing::Signed,
            AcceptedOffer {
                raw: raw.into(),
                option: option(),
                signature: Some("0xsig".into()),
                valid_until: None,
            },
        ));
        let back: CommercialEvidence =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.offer.unwrap().value.raw, raw);
    }
}
