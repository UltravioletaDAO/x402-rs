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

use alloy::network::TransactionBuilder as _;
use alloy::primitives::{Address, Bytes, FixedBytes, B256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolEvent, SolStruct};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::erc8004::proof::{verify_payment_facts, ProofRejection};
use crate::erc8004::ProofOfPayment;
use crate::network::Network;
use crate::payment_operator::abi::EscrowContract;
use crate::payment_operator::addresses::escrow_for_network;
use crate::payment_operator::types::EscrowPaymentInfo;

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
    /// The value moved out of an x402r escrow and no `escrowRelease` was
    /// supplied to say who funded it.
    ///
    /// Not the same failure as `PayerIsNotRecipient`, and collapsing them was
    /// costing us the whole escrow rail: on a release the ERC-20 `from` is the
    /// operator's TokenStore, never the buyer, so a perfectly honest anchor
    /// looked exactly like evidence hung off a stranger's payment. Measured on
    /// 23 of 23 live Execution Market releases sampled 2026-09-02.
    EscrowReleaseMissing,
    /// The transaction settled more than one escrow payment, so `paymentId`
    /// does not say which one this evidence is for.
    ///
    /// `paymentId` is `keccak256(caip2 || txHash)`, so every payment batched
    /// into one transaction collides on it. Certifying either would be a guess,
    /// and the wrong guess seals a stranger's delivery to a co-payer of the same
    /// batch. Refused; the anchor stays provisional, which is what it was
    /// anyway.
    EscrowReleaseAmbiguous,
    /// An `escrowRelease` was supplied and the chain does not agree with it.
    ///
    /// The authorization is checked by recomputing `getHash(paymentInfo)` on the
    /// escrow itself and requiring the result to be a `paymentInfoHash` that
    /// THIS transaction captured. A caller that edits a single field -- the
    /// payer above all -- changes the hash and lands here.
    EscrowReleaseInvalid,
    /// No seller signature.
    SellerSignatureMissing,
    /// The seller signature does not recover to the payee of the payment.
    SellerSignatureInvalid,
    /// The `proofOfPayment` is for a different transaction than the `paymentId`
    /// being claimed.
    ///
    /// A real payment proves a real payment -- it does not prove *which* payment
    /// the claim is about. Unbound, an attacker pays itself one wei and reuses
    /// that valid proof to claim any stranger's paymentId, passing every other
    /// check because it is genuinely both payer and payee of its own tiny
    /// transfer. Found by an audit 2026-08-19, on the fix for the previous
    /// version of this same hole.
    PaymentIdNotBound,
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
            AnchorRejection::EscrowReleaseMissing => "dx402_escrow_release_missing",
            AnchorRejection::EscrowReleaseAmbiguous => "dx402_escrow_release_ambiguous",
            AnchorRejection::EscrowReleaseInvalid => "dx402_escrow_release_invalid",
            AnchorRejection::SellerSignatureMissing => "dx402_seller_signature_missing",
            AnchorRejection::SellerSignatureInvalid => "dx402_seller_signature_invalid",
            AnchorRejection::PaymentIdNotBound => "dx402_payment_id_not_bound",
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
            // `UnverifiableChain` means "not checked here" -- reported, never
            // enforced, because refusing a check that never ran would silently
            // disable DX402 on every non-EVM family.
            ProofRejection::UnverifiableChain => AnchorRejection::UnverifiableChain,
            // `NotEvmTransaction` is the opposite: its own definition says "this
            // one DOES refuse" (erc8004/proof.rs). We only reach this arm after
            // resolving an EVM provider for the network, so a non-EVM tx hash
            // here is a definitely-bad proof, not an absent verdict. Lumping it
            // in above made it unenforceable in phase 2 -- the exact mistake
            // already documented for the ERC-8004 gate, where an ordering
            // change once masked a definite rejection as "we could not tell".
            ProofRejection::NotEvmTransaction => {
                AnchorRejection::Payment("proof_not_evm_transaction".into())
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

/// The x402r escrow authorization a payment was released from.
///
/// Supplied by the anchoring party when the money did not move directly from
/// buyer to seller. It is not trusted: `getHash` is recomputed on the escrow
/// contract and must equal a `paymentInfoHash` this very transaction captured,
/// which is what makes `payer` below an on-chain fact rather than a claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowRelease {
    /// The authorization struct, exactly as the escrow hashes it.
    pub payment_info: EscrowPaymentInfo,
    /// The buyer who funded the escrow. Part of the hashed struct, so editing
    /// it changes `getHash` and the claim is refused.
    pub payer: Address,
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
    /// Present when the payment was released from an x402r escrow.
    pub escrow_release: Option<&'a EscrowRelease>,
    pub chain_id: u64,
}

/// Who really bought this, when the ERC-20 `from` is an escrow.
///
/// # Why this exists
///
/// The gate binds the envelope's recipient to the payment: evidence must be
/// sealed to whoever paid. Reading that off the `Transfer` log is correct for
/// plain x402, where the buyer signs an EIP-3009 authorization and the buyer is
/// the `from`.
///
/// It is wrong for x402r escrow, and quietly so. On a release the tokens leave
/// the operator's **TokenStore** -- one contract per operator per chain -- so
/// every honest anchor on that rail reports a `from` that is not the buyer and
/// never can be. Sampled 2026-09-02 across Avalanche, Optimism and Monad: 23 of
/// 23 live Execution Market releases, all of them `payer_MISMATCH`. Left as it
/// was, phase 2 would have rejected the entire escrow rail as fraudulent.
///
/// # Why the answer is trustworthy
///
/// The escrow computes `paymentInfoHash` itself and emits it in
/// `PaymentCaptured` / `PaymentCharged`. So we ask the escrow to hash the
/// authorization we were handed and require the answer to be a hash **this
/// transaction** captured. That binds three things at once: the authorization is
/// authentic (any edited field changes the hash), it belongs to this payment
/// (the hash came out of this receipt), and it came from the escrow we know for
/// this network (we only read logs at that address).
///
/// Supplying no authorization is not an error to be papered over -- it means we
/// cannot tell who funded the escrow, and the anchor stays unverified.
async fn beneficial_payer<P: Provider>(
    rpc: &P,
    network: Network,
    tx_hash: FixedBytes<32>,
    release: Option<&EscrowRelease>,
    payee: Address,
) -> Result<Address, AnchorRejection> {
    let Some(release) = release else {
        return Err(AnchorRejection::EscrowReleaseMissing);
    };
    let Some(escrow) = escrow_for_network(network) else {
        // No x402r deployment on this chain, so an escrow release is not a thing
        // that can have happened here.
        return Err(AnchorRejection::EscrowReleaseMissing);
    };

    // The receiver in the authorization has to be the payee the payment proof
    // was checked against, or the two halves describe different payments and the
    // signature would be demanded from the wrong party.
    if release.payment_info.receiver != payee {
        return Err(AnchorRejection::EscrowReleaseInvalid);
    }

    let receipt = match rpc.get_transaction_receipt(tx_hash).await {
        Ok(Some(r)) => r,
        // `verify_payment_facts` already read this receipt and retried; reaching
        // here means the node stopped serving it mid-verification. No verdict,
        // not a rejection.
        Ok(None) => return Err(AnchorRejection::RpcUnavailable),
        Err(e) => {
            warn!(
                error = %crate::redact::scrub_urls(&e.to_string()),
                "escrow payer resolution could not read the receipt"
            );
            return Err(AnchorRejection::RpcUnavailable);
        }
    };

    // Every paymentInfoHash this transaction settled, from the escrow we know.
    // Both events carry it as topic 1; `charge` is the single-step path and
    // `capture` the two-step one, and Execution Market uses whichever the task
    // shape called for.
    let captured: Vec<B256> = receipt
        .inner
        .logs()
        .iter()
        .filter(|log| log.address() == escrow)
        .filter_map(|log| {
            EscrowContract::PaymentCaptured::decode_log(&log.inner)
                .map(|d| d.paymentInfoHash)
                .or_else(|_| {
                    EscrowContract::PaymentCharged::decode_log(&log.inner)
                        .map(|d| d.paymentInfoHash)
                })
                .ok()
        })
        .collect();
    let [captured_hash] = captured.as_slice() else {
        return Err(if captured.is_empty() {
            // The money did not leave a known escrow in this transaction, so
            // calling it an escrow release is simply false.
            AnchorRejection::EscrowReleaseInvalid
        } else {
            AnchorRejection::EscrowReleaseAmbiguous
        });
    };

    // Ask the escrow to hash what we were handed.
    let call = EscrowContract::getHashCall {
        paymentInfo: to_escrow_abi(&release.payment_info, release.payer),
    };
    let tx = TransactionRequest::default()
        .with_to(escrow)
        .with_input(Bytes::from(call.abi_encode()));
    let returned = match rpc.call(tx).await {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(
                error = %crate::redact::scrub_urls(&e.to_string()),
                "escrow payer resolution could not call getHash"
            );
            return Err(AnchorRejection::RpcUnavailable);
        }
    };
    let Ok(hash) = EscrowContract::getHashCall::abi_decode_returns(&returned) else {
        return Err(AnchorRejection::RpcUnavailable);
    };

    if *captured_hash != hash {
        return Err(AnchorRejection::EscrowReleaseInvalid);
    }
    Ok(release.payer)
}

/// The authorization in the shape the escrow ABI hashes.
///
/// `payer` is threaded in separately because it is not part of the wire struct
/// the merchant stack passes around -- the same split `EscrowLifecyclePayload`
/// already makes for `/escrow/state`.
fn to_escrow_abi(
    info: &EscrowPaymentInfo,
    payer: Address,
) -> crate::payment_operator::abi::EscrowPaymentInfo {
    use alloy::primitives::Uint;
    use alloy::primitives::U256;

    crate::payment_operator::abi::EscrowPaymentInfo {
        operator: info.operator,
        payer,
        receiver: info.receiver,
        token: info.token,
        maxAmount: Uint::from(info.max_amount),
        preApprovalExpiry: Uint::from(info.pre_approval_expiry),
        authorizationExpiry: Uint::from(info.authorization_expiry),
        refundExpiry: Uint::from(info.refund_expiry),
        minFeeBps: info.min_fee_bps,
        maxFeeBps: info.max_fee_bps,
        feeReceiver: info.fee_receiver,
        salt: U256::from_be_bytes(info.salt.0),
    }
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

    // The proof must be a proof OF THIS PAYMENT. Without this the gate verifies
    // a real payment and then certifies a paymentId that has nothing to do with
    // it: an attacker sends itself one wei, gets a perfectly valid proof where
    // payer == payee == itself, and presents it to claim a stranger's paymentId.
    // Every downstream check then passes -- the payer it sealed to is itself,
    // and the signature it made is over the payee the chain reports, itself --
    // so it reaches the FINAL rung and locks the real seller out forever.
    //
    // `paymentId` is a pure function of (network, txHash), so binding it is one
    // comparison. Local, and therefore before any RPC call.
    let bound = crate::dx402::payment_id(claim.network, &proof.transaction_hash.to_string());
    if !bound.eq_ignore_ascii_case(&format!("{:#x}", claim.payment_id)) {
        return Err(AnchorRejection::PaymentIdNotBound);
    }

    let sealed_to: Address = claim
        .sealed_to
        .clone()
        .try_into()
        .map_err(|_| AnchorRejection::UnverifiableChain)?;

    // Needed only on the escrow path below, but decoded here so the non-EVM
    // rejection stays a single, local decision rather than one made twice.
    let tx_hash: FixedBytes<32> = match proof.transaction_hash {
        crate::types::TransactionHash::Evm(bytes) => FixedBytes::from(bytes),
        _ => return Err(AnchorRejection::Payment("proof_not_evm_transaction".into())),
    };

    let facts = verify_payment_facts(rpc, claim.network, proof, anchor_max_age_secs()).await?;

    // The evidence must be sealed to whoever actually paid.
    //
    // On the escrow rail "whoever actually paid" is not the `from` of the
    // transfer -- that is the operator's TokenStore. Resolve the buyer through
    // the escrow's own authorization before concluding anything, and only then
    // compare. The direct rail is untouched: it matches on the first line and
    // never makes the extra calls.
    if facts.payer != sealed_to {
        let funder = beneficial_payer(
            rpc,
            claim.network,
            tx_hash,
            claim.escrow_release,
            facts.payee,
        )
        .await?;
        if funder != sealed_to {
            return Err(AnchorRejection::PayerIsNotRecipient);
        }
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
    fn a_definitely_bad_proof_is_enforceable_but_an_unread_chain_is_not() {
        // The two verdicts that never block exist so our own blind spots cannot
        // erase somebody's evidence. A proof we DID read and found wrong is not
        // one of them -- `NotEvmTransaction` documents itself as "this one DOES
        // refuse", and we only get here after resolving an EVM provider.
        let unread: AnchorRejection = ProofRejection::UnverifiableChain.into();
        assert!(!unread.is_enforceable(), "an unread chain must never block");

        let rpc = AnchorRejection::RpcUnavailable;
        assert!(!rpc.is_enforceable(), "our outage must never block");

        let bad: AnchorRejection = ProofRejection::NotEvmTransaction.into();
        assert!(
            bad.is_enforceable(),
            "a proof read and found invalid must be refusable in phase 2"
        );
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
    fn only_an_ABSENT_verdict_maps_to_unverifiable_chain() {
        // This test used to assert that `NotEvmTransaction` mapped to
        // `UnverifiableChain`, which made a definitely-bad proof unenforceable
        // in phase 2 -- pinning the bug as intended behaviour. Its own
        // definition in erc8004/proof.rs says "this one DOES refuse", and we
        // only reach the mapping after resolving an EVM provider.
        assert_eq!(
            AnchorRejection::from(ProofRejection::UnverifiableChain),
            AnchorRejection::UnverifiableChain
        );
        assert_eq!(
            AnchorRejection::from(ProofRejection::RpcUnavailable),
            AnchorRejection::RpcUnavailable
        );
        assert!(
            AnchorRejection::from(ProofRejection::NotEvmTransaction).is_enforceable(),
            "a proof we read and found invalid is a refusal, not a missing verdict"
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

    #[tokio::test]
    async fn a_proof_for_another_transaction_cannot_claim_this_payment() {
        // THE ATTACK: a real payment proves a real payment -- it does not prove
        // WHICH payment the claim is about. Mallory sends herself one wei, gets
        // a genuinely valid proof where she is both payer and payee, and points
        // it at Alice's paymentId. Every later check then passes: the payer
        // matches what she sealed to (herself), and her signature is over the
        // payee the chain reports (herself). She reaches the FINAL rung.
        //
        // The binding must also be LOCAL and run BEFORE the RPC -- an outage
        // must never turn a definite rejection into "we could not tell".
        // This test points at a dead port to prove it: if the check reached the
        // network we would get RpcUnavailable instead.
        use crate::erc8004::ProofOfPayment;

        let dead_rpc = alloy::providers::ProviderBuilder::new()
            .connect_http("http://127.0.0.1:1/".parse().unwrap());

        let mallorys_tx_hash = crate::types::TransactionHash::Evm([0xab; 32]);
        let mallorys_own_tx = mallorys_tx_hash.to_string();
        let proof = ProofOfPayment {
            transaction_hash: mallorys_tx_hash.clone(),
            block_number: 1,
            network: crate::network::Network::Base,
            payer: addr_of("0x1111111111111111111111111111111111111111"),
            payee: addr_of("0x1111111111111111111111111111111111111111"),
            amount: 1u64.into(),
            token: addr_of("0x2222222222222222222222222222222222222222"),
            timestamp: 0,
            payment_hash: b256(0),
        };

        // The paymentId of somebody else's payment, not of `mallorys_own_tx`.
        let alices_payment_id: B256 = crate::dx402::payment_id(
            crate::network::Network::Base,
            &format!("0x{}", "cd".repeat(32)),
        )
        .parse()
        .unwrap();

        let claim = AnchorClaim {
            network: crate::network::Network::Base,
            proof: Some(&proof),
            sealed_to: &addr_of("0x1111111111111111111111111111111111111111"),
            payment_id: alices_payment_id,
            content_hash: b256(9),
            pointer: "",
            seller_signature: Some("0x00"),
            escrow_release: None,
            chain_id: 8453,
        };

        assert_eq!(
            verify_anchor(&dead_rpc, &claim).await.unwrap_err(),
            AnchorRejection::PaymentIdNotBound,
            "a proof for another transaction must not certify this paymentId"
        );

        // And the honest case still gets past the binding: same proof, but the
        // paymentId that actually derives from its transaction.
        let bound: B256 = crate::dx402::payment_id(crate::network::Network::Base, &mallorys_own_tx)
            .parse()
            .unwrap();
        let ok_claim = AnchorClaim {
            payment_id: bound,
            ..claim
        };
        assert_ne!(
            verify_anchor(&dead_rpc, &ok_claim).await.unwrap_err(),
            AnchorRejection::PaymentIdNotBound,
            "a correctly bound paymentId must pass this check and go on to the chain"
        );
    }

    fn addr_of(s: &str) -> crate::types::MixedAddress {
        s.parse::<alloy::primitives::Address>().unwrap().into()
    }

    // ---------------------------------------------------------------------
    // The escrow rail
    // ---------------------------------------------------------------------

    /// A provider pointed at a closed port.
    ///
    /// Every escrow test below asserts a verdict this provider can never have
    /// contributed to: if one of them ever reached the network the answer would
    /// be `RpcUnavailable`, and the test would fail rather than quietly start
    /// depending on a chain.
    fn dead_provider() -> impl Provider {
        alloy::providers::ProviderBuilder::new()
            .connect_http("http://127.0.0.1:1/".parse().unwrap())
    }

    /// A real Execution Market release, kept as a fixture.
    ///
    /// Optimism `0x5a2822cc…`, block-verified 2026-09-02. The escrow
    /// (`0x320a3c35…`) emitted `PaymentCaptured` with
    /// `paymentInfoHash = 0xb54c89bf…`, and calling `getHash` on that escrow
    /// with the authorization below returns the same 32 bytes. Which is the
    /// whole mechanism in one line: the chain will confirm this buyer.
    fn em_release_fixture() -> (EscrowPaymentInfo, Address) {
        let info = EscrowPaymentInfo {
            operator: "0xc2377a9db1de2520bd6b2756ed012f4e82f7938e"
                .parse()
                .unwrap(),
            receiver: "0xf16f0882de08315b438e9f3a2abfb2d2e5d94eca"
                .parse()
                .unwrap(),
            token: "0x0b2c639c533813f4aa9d7837caf62653d097ff85"
                .parse()
                .unwrap(),
            max_amount: 20_000,
            pre_approval_expiry: 0x6a95_0d85,
            authorization_expiry: 0x6a9e_45ad,
            refund_expiry: 0x6aa7_802d,
            min_fee_bps: 0,
            max_fee_bps: 0x0708,
            fee_receiver: "0xc2377a9db1de2520bd6b2756ed012f4e82f7938e"
                .parse()
                .unwrap(),
            salt: "0x8a4644fd4909a144d006103fddc25bfc801fe32930e955e2450ba9711914a308"
                .parse()
                .unwrap(),
        };
        let payer: Address = "0x7fd9f9e51c9a94b3bca2082c8332cbf708b0b529"
            .parse()
            .unwrap();
        (info, payer)
    }

    #[test]
    fn the_escrow_call_goes_out_exactly_as_the_chain_expects_it() {
        // Pins the wire bytes of `getHash`, not just the field names. A swapped
        // pair of same-typed fields -- minFeeBps/maxFeeBps, or payer/receiver --
        // compiles, reads fine, and produces a hash that matches nothing, so
        // every honest escrow anchor would be refused as invalid. The expected
        // calldata is the 12 words the operator emitted for the fixture
        // transaction, which is what the escrow itself hashed.
        let (info, payer) = em_release_fixture();
        let call = EscrowContract::getHashCall {
            paymentInfo: to_escrow_abi(&info, payer),
        };
        let expected = concat!(
            "063a70ff",
            "000000000000000000000000c2377a9db1de2520bd6b2756ed012f4e82f7938e",
            "0000000000000000000000007fd9f9e51c9a94b3bca2082c8332cbf708b0b529",
            "000000000000000000000000f16f0882de08315b438e9f3a2abfb2d2e5d94eca",
            "0000000000000000000000000b2c639c533813f4aa9d7837caf62653d097ff85",
            "0000000000000000000000000000000000000000000000000000000000004e20",
            "000000000000000000000000000000000000000000000000000000006a950d85",
            "000000000000000000000000000000000000000000000000000000006a9e45ad",
            "000000000000000000000000000000000000000000000000000000006aa7802d",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000708",
            "000000000000000000000000c2377a9db1de2520bd6b2756ed012f4e82f7938e",
            "8a4644fd4909a144d006103fddc25bfc801fe32930e955e2450ba9711914a308",
        );
        assert_eq!(
            hex::encode(call.abi_encode()),
            expected,
            "the getHash calldata must reproduce the words the escrow hashed"
        );
    }

    #[tokio::test]
    async fn an_escrow_release_without_an_authorization_says_so() {
        // The distinction that makes the rail workable. Before this, an honest
        // Execution Market anchor and evidence hung off a stranger's payment
        // produced the SAME verdict, so the operator had no way to tell a
        // missing field from an attack.
        let (info, payer) = em_release_fixture();
        let rpc = dead_provider();

        let missing = beneficial_payer(
            &rpc,
            crate::network::Network::Optimism,
            alloy::primitives::FixedBytes::from([7u8; 32]),
            None,
            info.receiver,
        )
        .await
        .unwrap_err();
        assert_eq!(missing, AnchorRejection::EscrowReleaseMissing);
        assert!(
            missing.is_enforceable(),
            "a release with nobody named must be refusable in phase 2"
        );

        // Both verdicts are read off the chain, so both may block.
        assert!(AnchorRejection::EscrowReleaseInvalid.is_enforceable());
        let _ = payer;
    }

    #[tokio::test]
    async fn an_authorization_for_a_different_payee_is_refused_before_any_rpc() {
        // The authorization and the payment proof have to describe ONE payment.
        // Without this, a caller pairs a real escrow authorization it funded
        // with a proof of somebody else's transfer, and the signature would then
        // be demanded from the wrong party -- itself.
        let (info, payer) = em_release_fixture();
        let rpc = dead_provider();
        let release = EscrowRelease {
            payment_info: info,
            payer,
        };
        let other: Address = "0x000000000000000000000000000000000000dead"
            .parse()
            .unwrap();

        assert_eq!(
            beneficial_payer(
                &rpc,
                crate::network::Network::Optimism,
                alloy::primitives::FixedBytes::from([7u8; 32]),
                Some(&release),
                other,
            )
            .await
            .unwrap_err(),
            AnchorRejection::EscrowReleaseInvalid,
            "an authorization whose receiver is not the payee names another payment"
        );
    }

    #[tokio::test]
    async fn a_chain_with_no_escrow_deployment_cannot_have_had_an_escrow_release() {
        let (info, payer) = em_release_fixture();
        let rpc = dead_provider();
        let release = EscrowRelease {
            payment_info: info.clone(),
            payer,
        };
        assert_eq!(
            beneficial_payer(
                &rpc,
                // No x402r deployment here, so there is no escrow whose word we
                // could take for it.
                crate::network::Network::Bsc,
                alloy::primitives::FixedBytes::from([7u8; 32]),
                Some(&release),
                info.receiver,
            )
            .await
            .unwrap_err(),
            AnchorRejection::EscrowReleaseMissing
        );
    }

    #[test]
    fn a_batched_release_is_refused_rather_than_guessed() {
        // Two payments in one transaction share a paymentId, because paymentId
        // is a pure function of (network, txHash). Whichever we certified would
        // be a coin flip, and the losing side is a stranger whose delivery gets
        // sealed to a co-payer of the same batch. The verdict has to be its own,
        // or an operator reads "invalid" and goes hunting for a hash mismatch
        // that is not there.
        assert!(AnchorRejection::EscrowReleaseAmbiguous.is_enforceable());
        assert_eq!(
            AnchorRejection::EscrowReleaseAmbiguous.as_str(),
            "dx402_escrow_release_ambiguous"
        );
    }

    #[test]
    fn the_new_verdicts_have_stable_codes() {
        // Integrators branch on these strings.
        assert_eq!(
            AnchorRejection::EscrowReleaseMissing.as_str(),
            "dx402_escrow_release_missing"
        );
        assert_eq!(
            AnchorRejection::EscrowReleaseInvalid.as_str(),
            "dx402_escrow_release_invalid"
        );
    }
}
