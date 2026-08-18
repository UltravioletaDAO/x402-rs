//! Verifying that an anchor describes a payment that actually happened.
//!
//! # Why this exists
//!
//! `POST /dx402/anchor` shipped in v1.77.0 accepting any `paymentId` and any
//! `txHash`, checking nothing. Two things follow from that, and the second is
//! the serious one:
//!
//! - anyone can park bytes in the evidence store without paying;
//! - anyone can obtain a receipt **signed by this facilitator** for a payment
//!   that never existed.
//!
//! The receipt does not literally claim the payment occurred -- it says "this
//! was anchored for this paymentId" -- but that distinction is far too fine to
//! rely on anyone drawing.
//!
//! # What it reuses
//!
//! Everything about the payment itself:
//! [`crate::erc8004::proof::verify_payment_facts`]. That code already reads the
//! receipt, checks the block, the freshness and the `Transfer` log, and it was
//! written and reviewed for the ERC-8004 gate. A second copy would drift, and a
//! drifted payment check does not fail loudly -- it quietly accepts a payment
//! that never happened.
//!
//! # What is specific to DX402
//!
//! Two bindings that ERC-8004 solves differently:
//!
//! 1. **The payer must be the address the evidence was sealed to.** Without it,
//!    someone could seal evidence to their *own* key and hang it off somebody
//!    else's payment. This is the check that closes the real hole.
//! 2. **The anchor must be signed by the payee.** ERC-8004 ties payee to agent
//!    through the Identity Registry; there is no registry here, so the seller
//!    proves control of `payTo` by signing. Merely comparing the declared payee
//!    would leave a race: an observer of the transaction could anchor garbage
//!    first, and anti-replay would then lock out the legitimate seller. That
//!    turns the defence into a weapon.
//!
//! # Rollout
//!
//! `DX402_REQUIRE_PROOF` defaults to `false`: verify, report, do not reject.
//! Same discipline as `ERC8004_REQUIRE_PROOF`. Switching a gate on before the
//! logs show real traffic passing is how integrators who were working yesterday
//! stop working today.

use alloy::primitives::{Address, B256};
use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};
use serde::{Deserialize, Serialize};

use crate::erc8004::proof::{verify_payment_facts, ProofRejection};
use crate::erc8004::ProofOfPayment;
use crate::network::Network;

/// How old a payment may be when its evidence is anchored.
///
/// Fifteen minutes, against the seven days ERC-8004 allows. A rating can
/// legitimately arrive a week after the purchase; a DX402 anchor happens inside
/// the same request handler as the settle. A window wider than the operation
/// only widens the attack surface.
pub const DEFAULT_ANCHOR_MAX_AGE_SECS: u64 = 900;

/// Whether a failing proof blocks the anchor.
///
/// Phase 1 (`false`, the default) verifies and reports. Phase 2 rejects.
pub fn require_proof() -> bool {
    std::env::var("DX402_REQUIRE_PROOF")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn anchor_max_age_secs() -> u64 {
    std::env::var("DX402_ANCHOR_MAX_AGE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ANCHOR_MAX_AGE_SECS)
}

sol! {
    /// What the seller signs to prove the anchor is theirs.
    ///
    /// Binds the payment, the content and the location together. Signing only
    /// the `paymentId` would let a signature be lifted onto different content.
    #[derive(Debug)]
    struct Dx402AnchorAuthorization {
        bytes32 paymentId;
        bytes32 contentHash;
        string  pointer;
        address payee;
    }
}

/// Why an anchor was refused, or would have been in phase 2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorRejection {
    /// No `proofOfPayment` in the request.
    ProofMissing,
    /// The payment did not check out. Carries the underlying reason.
    Payment(String),
    /// The payer on the proof is not the address the evidence was sealed to.
    ///
    /// The one that matters most: without it, evidence sealed to an attacker's
    /// own key could be hung off a stranger's payment.
    PayerIsNotRecipient,
    /// No seller signature.
    SellerSignatureMissing,
    /// The seller signature does not recover to the payee of the payment.
    SellerSignatureInvalid,
    /// This payment already has evidence anchored.
    Replayed,
    /// Not an EVM chain, so there is no receipt to read.
    ///
    /// Reported, never enforced. Rejecting a check that never ran would
    /// silently disable DX402 on Solana, NEAR, Stellar and Algorand.
    UnverifiableChain,
    /// The RPC could not be reached, so no verdict was reached either.
    ///
    /// Distinct from a rejection on purpose: our outage must not be recorded as
    /// somebody's anchor being fraudulent.
    RpcUnavailable,
}

impl AnchorRejection {
    /// Whether this verdict may block the anchor in phase 2.
    ///
    /// Two never do. `RpcUnavailable` means no verdict was reached, and
    /// `UnverifiableChain` means the check does not apply to this family --
    /// enforcing either would turn an outage, or an entire chain, into a
    /// permanent refusal.
    pub fn is_enforceable(&self) -> bool {
        !matches!(
            self,
            AnchorRejection::RpcUnavailable | AnchorRejection::UnverifiableChain
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorRejection::ProofMissing => "dx402_proof_missing",
            AnchorRejection::Payment(_) => "dx402_proof_invalid",
            AnchorRejection::PayerIsNotRecipient => "dx402_payer_is_not_recipient",
            AnchorRejection::SellerSignatureMissing => "dx402_seller_signature_missing",
            AnchorRejection::SellerSignatureInvalid => "dx402_seller_signature_invalid",
            AnchorRejection::Replayed => "dx402_replayed",
            AnchorRejection::UnverifiableChain => "dx402_unverifiable_chain",
            AnchorRejection::RpcUnavailable => "dx402_rpc_unavailable",
        }
    }
}

impl From<ProofRejection> for AnchorRejection {
    fn from(r: ProofRejection) -> Self {
        match r {
            ProofRejection::RpcUnavailable => AnchorRejection::RpcUnavailable,
            ProofRejection::NotEvmTransaction | ProofRejection::UnverifiableChain => {
                AnchorRejection::UnverifiableChain
            }
            other => AnchorRejection::Payment(format!("{other:?}")),
        }
    }
}

/// The EIP-712 digest a seller signs to authorise an anchor.
pub fn authorization_digest(
    payment_id: B256,
    content_hash: B256,
    pointer: &str,
    payee: Address,
    chain_id: u64,
) -> B256 {
    let domain = eip712_domain! {
        name: "DX402 Anchor",
        version: "1",
        chain_id: chain_id,
    };
    Dx402AnchorAuthorization {
        paymentId: payment_id,
        contentHash: content_hash,
        pointer: pointer.to_string(),
        payee,
    }
    .eip712_signing_hash(&domain)
}

/// Sign an anchor authorization. Sellers use this; it lives here so both sides
/// derive the same digest from the same code.
pub fn sign_authorization(
    signer: &PrivateKeySigner,
    payment_id: B256,
    content_hash: B256,
    pointer: &str,
    chain_id: u64,
) -> Result<String, String> {
    use alloy::signers::SignerSync;
    let digest = authorization_digest(
        payment_id,
        content_hash,
        pointer,
        signer.address(),
        chain_id,
    );
    signer
        .sign_hash_sync(&digest)
        .map(|s| format!("0x{}", hex::encode(s.as_bytes())))
        .map_err(|e| e.to_string())
}

/// Check that a seller signature proves control of `expected_payee`.
///
/// Dispatches on the payee's own curve, because "prove you control the address
/// that got paid" means different arithmetic per chain and the point is the
/// claim, not the algorithm:
///
/// - **EVM** (secp256k1): recover the signer from the EIP-712 digest.
/// - **Solana / ed25519 families**: verify a raw ed25519 signature over the same
///   digest, against the public key the address already is.
///
/// The ed25519 path exists because a Solana seller cannot produce an EIP-712
/// signature at all — its payee is an ed25519 address. Requiring one would have
/// left Solana permanently unable to prove authorship, which is exactly the hole
/// this check exists to close. Raised by KarmaKadabra, 2026-08-18.
///
/// Verifiable with no RPC on either curve, which is what lets it be enforced
/// even while the on-chain half of the gate is still reporting-only.
pub fn verify_authorization_for(
    payee: &crate::types::MixedAddress,
    signature: &str,
    payment_id: B256,
    content_hash: B256,
    pointer: &str,
    chain_id: u64,
) -> bool {
    use crate::types::MixedAddress;

    let Ok(raw) = hex::decode(signature.trim_start_matches("0x")) else {
        return false;
    };

    match payee {
        MixedAddress::Evm(addr) => {
            let expected: Address = (*addr).into();
            let Ok(sig) = alloy::primitives::Signature::try_from(raw.as_slice()) else {
                return false;
            };
            let digest =
                authorization_digest(payment_id, content_hash, pointer, expected, chain_id);
            sig.recover_address_from_prehash(&digest)
                .map(|a| a == expected)
                .unwrap_or(false)
        }
        MixedAddress::Solana(pubkey) => verify_ed25519(
            &pubkey.to_bytes(),
            &raw,
            payment_id,
            content_hash,
            pointer,
            chain_id,
        ),
        MixedAddress::Stellar(addr) => {
            match stellar_strkey::ed25519::PublicKey::from_string(addr) {
                Ok(pk) => verify_ed25519(&pk.0, &raw, payment_id, content_hash, pointer, chain_id),
                Err(_) => false,
            }
        }
        // NEAR account ids, Sui hashes and the rest cannot be turned into a
        // verifying key from the address alone. Reported honestly as "not
        // proven" rather than quietly accepted.
        _ => false,
    }
}

/// Verify a raw ed25519 signature over the anchor digest.
///
/// The digest is the *same* EIP-712 hash the secp256k1 path uses, so there is
/// one canonical message across curves rather than a second thing to keep in
/// sync. `payee` is the zero address inside it: an ed25519 address does not fit
/// the `address` field, and the binding to the payee is already established by
/// which public key verifies the signature.
fn verify_ed25519(
    pubkey: &[u8],
    signature: &[u8],
    payment_id: B256,
    content_hash: B256,
    pointer: &str,
    chain_id: u64,
) -> bool {
    let Ok(pubkey): Result<[u8; 32], _> = pubkey.try_into() else {
        return false;
    };
    let Ok(signature): Result<[u8; 64], _> = signature.try_into() else {
        return false;
    };
    let Ok(verifying) = ed25519_dalek::VerifyingKey::from_bytes(&pubkey) else {
        return false;
    };

    let digest = authorization_digest(payment_id, content_hash, pointer, Address::ZERO, chain_id);
    use ed25519_dalek::Verifier;
    verifying
        .verify(
            digest.as_slice(),
            &ed25519_dalek::Signature::from_bytes(&signature),
        )
        .is_ok()
}

/// Check that a seller signature recovers to `expected_payee` (EVM only).
pub fn verify_authorization(
    signature: &str,
    payment_id: B256,
    content_hash: B256,
    pointer: &str,
    expected_payee: Address,
    chain_id: u64,
) -> bool {
    let Ok(raw) = hex::decode(signature.trim_start_matches("0x")) else {
        return false;
    };
    let Ok(sig) = alloy::primitives::Signature::try_from(raw.as_slice()) else {
        return false;
    };
    let digest = authorization_digest(payment_id, content_hash, pointer, expected_payee, chain_id);
    sig.recover_address_from_prehash(&digest)
        .map(|a| a == expected_payee)
        .unwrap_or(false)
}

/// Everything the gate needs to judge one anchor.
pub struct AnchorClaim<'a> {
    pub network: Network,
    pub proof: Option<&'a ProofOfPayment>,
    /// The address the evidence was sealed to.
    pub sealed_to: &'a crate::types::MixedAddress,
    pub payment_id: B256,
    pub content_hash: B256,
    pub pointer: &'a str,
    pub seller_signature: Option<&'a str>,
    pub chain_id: u64,
}

/// Verify an anchor claim.
///
/// Returns `Ok(())` when the anchor is backed by a real payment made by the
/// address the evidence was sealed to, and authorised by the payee.
pub async fn verify_anchor<P: Provider>(
    rpc: &P,
    claim: &AnchorClaim<'_>,
) -> Result<(), AnchorRejection> {
    let Some(proof) = claim.proof else {
        return Err(AnchorRejection::ProofMissing);
    };

    // Local checks first, before any RPC call. `RpcUnavailable` is the verdict
    // that reaches no conclusion, so anything decidable without the network has
    // to be decided here -- otherwise an outage masks a definite rejection as
    // "we could not tell". This ordering is not cosmetic; the equivalent
    // refactor in the ERC-8004 gate regressed exactly this way and two tests
    // caught it.
    let Some(signature) = claim.seller_signature else {
        return Err(AnchorRejection::SellerSignatureMissing);
    };

    let sealed_to: Address = claim
        .sealed_to
        .clone()
        .try_into()
        .map_err(|_| AnchorRejection::UnverifiableChain)?;

    let facts = verify_payment_facts(rpc, claim.network, proof, anchor_max_age_secs()).await?;

    // The evidence must be sealed to whoever actually paid.
    if facts.payer != sealed_to {
        return Err(AnchorRejection::PayerIsNotRecipient);
    }

    // And the anchor must come from whoever got paid.
    if !verify_authorization(
        signature,
        claim.payment_id,
        claim.content_hash,
        claim.pointer,
        facts.payee,
        claim.chain_id,
    ) {
        return Err(AnchorRejection::SellerSignatureInvalid);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b256(byte: u8) -> B256 {
        B256::from([byte; 32])
    }

    #[test]
    fn a_signature_verifies_against_its_signer() {
        let signer = PrivateKeySigner::random();
        let sig = sign_authorization(&signer, b256(1), b256(2), "s3+https://x/y", 8453).unwrap();
        assert!(verify_authorization(
            &sig,
            b256(1),
            b256(2),
            "s3+https://x/y",
            signer.address(),
            8453
        ));
    }

    #[test]
    fn every_field_is_bound_into_the_signature() {
        // A signature over the paymentId alone could be lifted onto different
        // content, which would let a seller authorise an anchor and then have
        // something else stored under it.
        let signer = PrivateKeySigner::random();
        let sig = sign_authorization(&signer, b256(1), b256(2), "s3+https://x/y", 8453).unwrap();
        let a = signer.address();

        assert!(!verify_authorization(
            &sig,
            b256(9),
            b256(2),
            "s3+https://x/y",
            a,
            8453
        ));
        assert!(!verify_authorization(
            &sig,
            b256(1),
            b256(9),
            "s3+https://x/y",
            a,
            8453
        ));
        assert!(!verify_authorization(
            &sig,
            b256(1),
            b256(2),
            "s3+https://evil/y",
            a,
            8453
        ));
        assert!(!verify_authorization(
            &sig,
            b256(1),
            b256(2),
            "s3+https://x/y",
            a,
            84532
        ));
    }

    #[test]
    fn another_sellers_signature_does_not_authorise_it() {
        let signer = PrivateKeySigner::random();
        let impostor = PrivateKeySigner::random();
        let sig = sign_authorization(&signer, b256(1), b256(2), "p", 8453).unwrap();
        assert!(!verify_authorization(
            &sig,
            b256(1),
            b256(2),
            "p",
            impostor.address(),
            8453
        ));
    }

    #[test]
    fn malformed_signatures_are_rejected_not_panicked_on() {
        for bad in ["", "0x", "0xzz", "0x00", "not-hex"] {
            assert!(!verify_authorization(
                bad,
                b256(1),
                b256(2),
                "p",
                Address::ZERO,
                8453
            ));
        }
    }

    #[test]
    fn a_solana_seller_can_prove_authorship_with_ed25519() {
        // The path that makes the fix real on Solana. A Solana payee is an
        // ed25519 address and cannot produce an EIP-712 signature at all, so
        // requiring one would leave that chain permanently unable to prove
        // authorship -- which is the hole the check exists to close.
        use ed25519_dalek::Signer;

        let seed = [0x37u8; 32];
        let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
        let address = bs58::encode(signing.verifying_key().to_bytes()).into_string();
        let payee: crate::types::MixedAddress =
            serde_json::from_value(serde_json::Value::String(address)).unwrap();

        let digest = authorization_digest(b256(1), b256(2), "p", Address::ZERO, 0);
        let sig = format!(
            "0x{}",
            hex::encode(signing.sign(digest.as_slice()).to_bytes())
        );

        assert!(verify_authorization_for(
            &payee,
            &sig,
            b256(1),
            b256(2),
            "p",
            0
        ));

        // And it is bound to the content, not just the payment.
        assert!(!verify_authorization_for(
            &payee,
            &sig,
            b256(1),
            b256(9),
            "p",
            0
        ));
        assert!(!verify_authorization_for(
            &payee,
            &sig,
            b256(1),
            b256(2),
            "otro",
            0
        ));
    }

    #[test]
    fn another_solana_wallet_cannot_claim_the_anchor() {
        use ed25519_dalek::Signer;

        let mine = ed25519_dalek::SigningKey::from_bytes(&[0x37u8; 32]);
        let theirs = ed25519_dalek::SigningKey::from_bytes(&[0x99u8; 32]);
        let their_address = bs58::encode(theirs.verifying_key().to_bytes()).into_string();
        let payee: crate::types::MixedAddress =
            serde_json::from_value(serde_json::Value::String(their_address)).unwrap();

        let digest = authorization_digest(b256(1), b256(2), "p", Address::ZERO, 0);
        let sig = format!("0x{}", hex::encode(mine.sign(digest.as_slice()).to_bytes()));

        assert!(!verify_authorization_for(
            &payee,
            &sig,
            b256(1),
            b256(2),
            "p",
            0
        ));
    }

    #[test]
    fn an_address_with_no_verifying_key_is_reported_as_unproven() {
        // NEAR account ids and Sui hashes cannot be turned into a key from the
        // address alone. Say "not proven" rather than quietly accepting.
        let near: crate::types::MixedAddress =
            serde_json::from_value(serde_json::Value::String("uvd-facilitator.near".into()))
                .unwrap();
        assert!(!verify_authorization_for(
            &near,
            "0xdead",
            b256(1),
            b256(2),
            "p",
            0
        ));
    }

    #[test]
    fn outages_and_foreign_chains_never_block() {
        // The two verdicts that mean "no conclusion" rather than "fraud".
        // Enforcing either would turn an RPC blip, or every non-EVM chain, into
        // a permanent refusal.
        assert!(!AnchorRejection::RpcUnavailable.is_enforceable());
        assert!(!AnchorRejection::UnverifiableChain.is_enforceable());

        for r in [
            AnchorRejection::ProofMissing,
            AnchorRejection::Payment("x".into()),
            AnchorRejection::PayerIsNotRecipient,
            AnchorRejection::SellerSignatureMissing,
            AnchorRejection::SellerSignatureInvalid,
            AnchorRejection::Replayed,
        ] {
            assert!(r.is_enforceable(), "{r:?} should be enforceable");
        }
    }

    #[test]
    fn non_evm_proof_rejections_map_to_unverifiable_chain() {
        assert_eq!(
            AnchorRejection::from(ProofRejection::NotEvmTransaction),
            AnchorRejection::UnverifiableChain
        );
        assert_eq!(
            AnchorRejection::from(ProofRejection::RpcUnavailable),
            AnchorRejection::RpcUnavailable
        );
    }

    #[test]
    fn the_default_window_is_much_tighter_than_erc8004() {
        // The anchor happens inside the same handler as the settle. A window
        // wider than the operation only widens the attack surface.
        assert_eq!(DEFAULT_ANCHOR_MAX_AGE_SECS, 900);
        assert!(DEFAULT_ANCHOR_MAX_AGE_SECS < 604_800);
    }

    #[test]
    fn the_gate_is_off_until_explicitly_enabled() {
        // Phase 1: verify and report. Turning this on before the logs show real
        // traffic passing breaks integrators who were working yesterday.
        let prior = std::env::var("DX402_REQUIRE_PROOF").ok();
        std::env::remove_var("DX402_REQUIRE_PROOF");
        assert!(!require_proof());
        if let Some(v) = prior {
            std::env::set_var("DX402_REQUIRE_PROOF", v);
        }
    }
}
