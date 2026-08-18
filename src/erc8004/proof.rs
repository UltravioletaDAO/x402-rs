//! Server-side verification of the `ProofOfPayment` carried by a feedback
//! submission.
//!
//! # Why this exists
//!
//! The ERC-8004 Reputation Registry lets ANY address rate ANY agent. Until this
//! module existed, the only thing standing between the registry and a sybil
//! flood was the fact that the facilitator signs every feedback itself — that
//! is, our centralisation was working as de-facto access control. Fixing
//! authorship without a real gate would have opened the flood.
//!
//! `ProofOfPayment` already travelled in the settle response and
//! `FeedbackParams.proof` already documented itself as *"required for authorized
//! feedback"* — but nothing ever read it. It was produced, published field by
//! field in `GET /feedback`, and dropped.
//!
//! # What a passing proof does and does not establish
//!
//! It establishes that a real, successful, on-chain payment of `amount` in
//! `token` moved from `payer` to an address the registry associates with
//! `agentId`, recently, on the network the feedback is being written to, and
//! that this exact (payment, agent) pair has not been cashed in for a rating
//! before.
//!
//! It does **not** establish that the caller IS the payer. Nothing in the
//! request is signed by the rater yet, so a third party who observes a proof can
//! replay it — once, against a different agent, until the anti-replay claim
//! catches the pair. That gap closes with real authorship (P2: partially-signed
//! transactions on SVM, EIP-7702 delegation on EVM), not here. Saying it out
//! loud matters: the point of this module is to stop over-claiming what the
//! facilitator has checked, so it must not start over-claiming in its own turn.
//!
//! # Anchoring is not verifying
//!
//! `giveFeedback` carries `feedbackURI` + `feedbackHash`, and that hash is the
//! keccak256 of the off-chain document. If the payment is described inside the
//! document, it is anchored on-chain — cryptographically bound, with the ABI
//! that already exists. But the chain stores a hash; it never checks what the
//! document claims. Anchoring gives integrity, this gate gives veracity, and the
//! two are separate numbers: "carries the tx anchored" and "the document is
//! retrievable and its keccak matches the on-chain `feedbackHash`".

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{keccak256, Address, FixedBytes, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolEvent;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::erc8004::abi::IIdentityRegistry;
use crate::erc8004::types::{FeedbackParams, ProofOfPayment};
use crate::network::Network;
use crate::types::{MixedAddress, TransactionHash};

sol! {
    /// The ERC-20 `Transfer` event. Declared here rather than reused from the
    /// USDC bindings so this module does not depend on which token ABI happens
    /// to be loaded: every EIP-3009 token we settle emits this exact signature.
    event Transfer(address indexed from, address indexed to, uint256 value);
}

/// Turns the gate from "verify and log" into "verify and reject".
pub const ENV_REQUIRE_PROOF: &str = "ERC8004_REQUIRE_PROOF";

/// How old a payment may be and still buy a rating.
pub const ENV_PROOF_MAX_AGE_SECS: &str = "ERC8004_PROOF_MAX_AGE_SECS";

/// Seven days. Long enough that a client can rate an interaction the following
/// week, short enough that a proof is not a permanent bearer token.
pub const DEFAULT_PROOF_MAX_AGE_SECS: u64 = 7 * 24 * 3600;

/// Tolerance for a block timestamp that reads slightly ahead of our clock.
const FUTURE_SKEW_TOLERANCE_SECS: u64 = 300;

/// How long we refuse to let the same (payment, agent) pair be rated again.
///
/// Deliberately tied to the freshness window: once a proof is too old to be
/// accepted, it no longer needs a replay record to stop it, so a TTL shorter
/// than the freshness window would open a gap and a longer one would only
/// store rows that the freshness check already rejects.
pub fn replay_ttl_secs() -> u64 {
    proof_max_age_secs()
}

/// Phase 2 of the rollout. Default `false`: verify and log the verdict without
/// rejecting, so we can measure how much real traffic a hard gate would break
/// before it breaks it.
pub fn is_proof_required() -> bool {
    std::env::var(ENV_REQUIRE_PROOF)
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

/// Freshness window, from the environment or [`DEFAULT_PROOF_MAX_AGE_SECS`].
pub fn proof_max_age_secs() -> u64 {
    std::env::var(ENV_PROOF_MAX_AGE_SECS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_PROOF_MAX_AGE_SECS)
}

/// Why a proof was refused.
///
/// A BOUNDED set, never the raw error. Raw RPC and contract errors carry payer
/// addresses and sometimes RPC URLs with the API key inside them; this enum is
/// what reaches the client and the event stream, and `src/redact.rs` exists
/// because that lesson was already learned once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofRejection {
    /// No `proof` in the request at all.
    Missing,
    /// The request carries no `rater`, so "payer == rater" is not decidable.
    RaterMissing,
    /// The proof describes a payment on a different chain than the feedback.
    NetworkMismatch,
    /// The FEEDBACK is being written on a chain this gate cannot read (SVM).
    /// Never blocks a write: it means "not checked here", not "refused".
    UnverifiableChain,
    /// The proof's transaction hash is not an EVM hash, so there is nothing to
    /// read on the chain the feedback is going to. This one DOES refuse.
    NotEvmTransaction,
    /// `rater` or an address inside the proof is not an EVM address.
    NotEvmAddress,
    /// No such transaction on that chain.
    TransactionNotFound,
    /// The transaction exists but reverted.
    TransactionReverted,
    /// The receipt sits in a different block than the proof claims.
    BlockNumberMismatch,
    /// The block's own timestamp is not the one the proof declares.
    TimestampMismatch,
    /// No matching ERC-20 `Transfer` in that transaction.
    TransferNotFound,
    /// The payment was made by somebody other than the rater.
    PayerIsNotRater,
    /// The payment did not go to an address the registry ties to this agent.
    PayeeIsNotAgent,
    /// The payment is older than the freshness window.
    Expired,
    /// `paymentHash` does not match the fields it commits to.
    PaymentHashMismatch,
    /// This (payment, agent) pair already bought a rating.
    Replayed,
    /// The off-chain document does not hash to the declared `feedbackHash`.
    DocumentHashMismatch,
    /// The document is retrievable but does not mention this payment.
    DocumentPaymentMismatch,
    /// We could not reach the chain to decide. Distinct from a refusal: it is
    /// "no verdict", and it must never be recorded as "not paid".
    RpcUnavailable,
}

impl ProofRejection {
    /// Stable snake_case token for logs, events, and API responses.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Missing => "proof_missing",
            Self::RaterMissing => "rater_missing",
            Self::NetworkMismatch => "proof_network_mismatch",
            Self::UnverifiableChain => "proof_unverifiable_chain",
            Self::NotEvmTransaction => "proof_not_evm_transaction",
            Self::NotEvmAddress => "proof_not_evm_address",
            Self::TransactionNotFound => "proof_transaction_not_found",
            Self::TransactionReverted => "proof_transaction_reverted",
            Self::BlockNumberMismatch => "proof_block_number_mismatch",
            Self::TimestampMismatch => "proof_timestamp_mismatch",
            Self::TransferNotFound => "proof_transfer_not_found",
            Self::PayerIsNotRater => "proof_payer_is_not_rater",
            Self::PayeeIsNotAgent => "proof_payee_is_not_agent",
            Self::Expired => "proof_expired",
            Self::PaymentHashMismatch => "proof_payment_hash_mismatch",
            Self::Replayed => "proof_replayed",
            Self::DocumentHashMismatch => "feedback_document_hash_mismatch",
            Self::DocumentPaymentMismatch => "feedback_document_payment_mismatch",
            Self::RpcUnavailable => "proof_rpc_unavailable",
        }
    }

    /// True when the caller may reasonably retry the same request unchanged.
    ///
    /// Only "we reached no verdict" is retryable. Collapsing this into a plain
    /// refusal is how a transient RPC failure becomes a permanent wrong answer
    /// (INC-2026-07-21).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RpcUnavailable)
    }

    /// Should this reason stop the feedback from being written, once the gate
    /// is enforced?
    ///
    /// Two reasons never stop it, and for different sentences:
    ///   * `RpcUnavailable` is "no verdict" -- refusing on it would turn our own
    ///     outage into somebody else's missing reputation;
    ///   * `UnverifiableChain` is "not checked on this path" -- the SVM
    ///     feedback route has no EVM receipt to read, and enforcing a check
    ///     that was never run would silently disable Solana reputation.
    /// Everything else is an actual refusal.
    pub fn blocks_write(&self) -> bool {
        !matches!(self, Self::RpcUnavailable | Self::UnverifiableChain)
    }
}

/// State of the on-chain anchor: is the document the `feedbackHash` commits to
/// actually retrievable, and does it hash to that value?
///
/// Separate from the verdict on purpose. A document nobody can fetch does not
/// make the payment fake — it makes the anchor decorative, which is a different
/// (and separately countable) problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
    /// No `feedbackURI`/`feedbackHash` pair was supplied.
    NotDeclared,
    /// Fetched, hashes to `feedbackHash`, and names this payment.
    Auditable,
    /// Fetched and hashes correctly, but does not mention the payment.
    HashOnly,
    /// A hash was declared but the document could not be fetched.
    Unreachable,
    /// The document was fetched and does NOT hash to `feedbackHash`.
    Mismatch,
}

impl AnchorStatus {
    /// Stable snake_case token for logs and API responses.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotDeclared => "not_declared",
            Self::Auditable => "auditable",
            Self::HashOnly => "hash_only",
            Self::Unreachable => "unreachable",
            Self::Mismatch => "mismatch",
        }
    }
}

/// Everything the gate learned about one feedback submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofReport {
    /// `None` when the proof verified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<ProofRejection>,
    /// State of the document anchor, independent of the verdict.
    pub anchor: AnchorStatus,
    /// Whether a failing verdict actually blocks the write, i.e. whether
    /// `ERC8004_REQUIRE_PROOF` was on when this was evaluated.
    pub enforced: bool,
}

impl ProofReport {
    /// Did the proof verify?
    pub fn is_verified(&self) -> bool {
        self.rejection.is_none()
    }

    /// Should this submission be refused?
    ///
    /// Only in phase 2, and never for "we reached no verdict": refusing on
    /// `RpcUnavailable` would turn our own outage into somebody else's missing
    /// reputation.
    pub fn should_reject(&self) -> bool {
        match self.rejection {
            Some(r) => self.enforced && r.blocks_write(),
            None => false,
        }
    }
}

/// Verify the proof attached to a feedback submission.
///
/// Every check below is necessary and none is sufficient alone, which is why
/// they are all here rather than sampled.
pub async fn evaluate_feedback_proof<P: Provider>(
    rpc: &P,
    identity_registry: Address,
    request_network: Network,
    agent_id: u64,
    params: &FeedbackParams,
) -> ProofReport {
    let enforced = is_proof_required();
    let Some(proof) = params.proof.as_ref() else {
        return ProofReport {
            rejection: Some(ProofRejection::Missing),
            anchor: AnchorStatus::NotDeclared,
            enforced,
        };
    };

    let verdict = verify_proof_of_payment(
        rpc,
        identity_registry,
        request_network,
        agent_id,
        params.rater.as_ref(),
        proof,
    )
    .await;

    // The anchor is evaluated even when the payment verdict failed: the two
    // numbers are reported separately and one does not gate the other.
    let anchor = evaluate_anchor(params, proof).await;

    let rejection = match verdict {
        Err(r) => Some(r),
        // A mismatching document is a refusal in its own right: the on-chain
        // hash would commit to something other than what we were shown.
        Ok(()) if anchor == AnchorStatus::Mismatch => Some(ProofRejection::DocumentHashMismatch),
        Ok(()) => None,
    };

    ProofReport {
        rejection,
        anchor,
        enforced,
    }
}

/// Evaluate a feedback that is being written on Solana.
///
/// The document half of the gate is chain-agnostic and still runs: the keccak of
/// the off-chain document does not care which chain anchors it. The payment half
/// does not -- verifying an SVM payment means reading an SVM transaction, which
/// this module does not do -- so the verdict is `UnverifiableChain`, which by
/// construction never blocks the write. This is an honest gap, recorded as one,
/// rather than a check that pretends to have run.
pub async fn evaluate_svm_feedback_proof(params: &FeedbackParams) -> ProofReport {
    let enforced = is_proof_required();
    let anchor = match params.proof.as_ref() {
        Some(proof) => evaluate_anchor(params, proof).await,
        None => anchor_without_payment(params).await,
    };
    let rejection = match params.proof {
        None => Some(ProofRejection::Missing),
        Some(_) => Some(ProofRejection::UnverifiableChain),
    };
    ProofReport {
        rejection,
        anchor,
        enforced,
    }
}

/// The payment half of the gate: does this proof describe a real payment from
/// the rater to this agent?
/// The payment-level half of proof verification, shared by every consumer.
///
/// Establishes that a real, successful, on-chain transfer of `amount` in `token`
/// moved from `payer` to `payee`, in the block the proof names, on the network
/// the caller expects, recently enough.
///
/// It deliberately says nothing about **who may cash the proof in**. ERC-8004
/// requires the payer to be the rater and the payee to be the agent; DX402
/// requires the payer to be the address the evidence was sealed to and the payee
/// to have signed the anchor. Those are the callers' business.
///
/// Factored out rather than copied: a second implementation of these seven
/// checks would drift, and a drifted payment check does not fail loudly -- it
/// quietly accepts a payment that never happened.
///
/// `max_age_secs` is a parameter because the right window differs by caller.
/// A rating can legitimately arrive a week after the purchase; a DX402 anchor
/// happens inside the same handler as the settle, so it gets minutes.
pub async fn verify_payment_facts<P: Provider>(
    rpc: &P,
    request_network: Network,
    proof: &ProofOfPayment,
    max_age_secs: u64,
) -> Result<PaymentFacts, ProofRejection> {
    // 1. Same chain the caller expects. Without this, a payment on a cheap chain
    //    would buy something on an expensive one.
    if proof.network != request_network {
        return Err(ProofRejection::NetworkMismatch);
    }

    // 2. The proof must recompute. `payment_hash` commits to tx hash, block,
    //    payer, payee and amount, so a mismatch means the struct was edited
    //    after the settle produced it.
    if proof.recompute_payment_hash() != proof.payment_hash {
        return Err(ProofRejection::PaymentHashMismatch);
    }

    let tx_hash: FixedBytes<32> = match proof.transaction_hash {
        TransactionHash::Evm(bytes) => FixedBytes::from(bytes),
        _ => return Err(ProofRejection::NotEvmTransaction),
    };

    let payer = evm_address(&proof.payer)?;
    let payee = evm_address(&proof.payee)?;
    let token = evm_address(&proof.token)?;

    // 3. The transaction exists and succeeded.
    let receipt = match rpc.get_transaction_receipt(tx_hash).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(ProofRejection::TransactionNotFound),
        Err(e) => {
            // Scrubbed: alloy transport errors embed the full RPC URL, API key
            // included.
            warn!(
                error = %crate::redact::scrub_urls(&e.to_string()),
                "proof verification could not read the receipt"
            );
            return Err(ProofRejection::RpcUnavailable);
        }
    };
    if !receipt.status() {
        return Err(ProofRejection::TransactionReverted);
    }

    // 4. Same block the proof claims.
    if receipt.block_number != Some(proof.block_number) {
        return Err(ProofRejection::BlockNumberMismatch);
    }

    // 5. Freshness, measured against the BLOCK's timestamp rather than the
    //    caller's. `payment_hash` does not commit to `timestamp`, so a
    //    caller-supplied one is unauthenticated and a stale proof could be made
    //    to look fresh simply by rewriting the field.
    let block_ts = match rpc.get_block_by_number(proof.block_number.into()).await {
        Ok(Some(b)) => b.header.timestamp,
        Ok(None) => return Err(ProofRejection::BlockNumberMismatch),
        Err(e) => {
            warn!(
                error = %crate::redact::scrub_urls(&e.to_string()),
                "proof verification could not read the block"
            );
            return Err(ProofRejection::RpcUnavailable);
        }
    };
    if block_ts != proof.timestamp {
        return Err(ProofRejection::TimestampMismatch);
    }
    let now = unix_now();
    if block_ts > now.saturating_add(FUTURE_SKEW_TOLERANCE_SECS) {
        return Err(ProofRejection::TimestampMismatch);
    }
    if now.saturating_sub(block_ts) > max_age_secs {
        return Err(ProofRejection::Expired);
    }

    // 6. The transfer the proof describes is actually in that transaction.
    //    Reading the logs (not the calldata) is what makes this robust to
    //    however the payment was routed: EIP-3009, a proxy, or a batch all emit
    //    the same event.
    let amount: U256 = proof.amount.into();
    let found = receipt.inner.logs().iter().any(|log| {
        if log.address() != token {
            return false;
        }
        match Transfer::decode_log(&log.inner) {
            Ok(decoded) => decoded.from == payer && decoded.to == payee && decoded.value == amount,
            Err(_) => false,
        }
    });
    if !found {
        return Err(ProofRejection::TransferNotFound);
    }

    Ok(PaymentFacts { payer, payee })
}

/// Who the verified payment moved value between.
///
/// Returned so a caller does not have to re-parse the addresses it just had
/// checked, and cannot accidentally check one address and then authorise
/// against another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaymentFacts {
    pub payer: Address,
    pub payee: Address,
}

pub async fn verify_proof_of_payment<P: Provider>(
    rpc: &P,
    identity_registry: Address,
    request_network: Network,
    agent_id: u64,
    rater: Option<&MixedAddress>,
    proof: &ProofOfPayment,
) -> Result<(), ProofRejection> {
    // The rater checks run BEFORE any RPC call, and the order is load-bearing.
    //
    // They need nothing but the request itself, and `RpcUnavailable` is the one
    // verdict that does NOT block a write -- it means "no verdict reached".
    // Checking them after the network calls would let an RPC outage turn a
    // definite "wrong rater" into "we could not tell", which is exactly how a
    // proof for somebody else's payment would slip through phase 2. Two tests
    // pin this ordering; they caught it when the shared verification was
    // factored out.
    let Some(rater) = rater else {
        return Err(ProofRejection::RaterMissing);
    };
    let rater = evm_address(rater)?;
    if evm_address(&proof.payer)? != rater {
        return Err(ProofRejection::PayerIsNotRater);
    }

    let facts = verify_payment_facts(rpc, request_network, proof, proof_max_age_secs()).await?;
    let payee = facts.payee;

    // The money went to this agent.
    //
    // MEASURED, not assumed (2026-08-13, Base mainnet, identity registry
    // 0x8004A169...): `getAgentWallet(agentId)` returns the zero address for
    // most agents -- 18896, 58517, 100, 1000, 5000 and 40000 all read 0x0 --
    // while `ownerOf(agentId)` always answers. Where a wallet IS set (agents 1
    // and 60000) it equals the owner. So the wallet alone would reject nearly
    // every real payment: both are accepted, with the explicitly declared
    // payment wallet taking precedence when it exists.
    let identity = IIdentityRegistry::new(identity_registry, rpc);
    let owner = match identity.ownerOf(U256::from(agent_id)).call().await {
        Ok(a) => Some(a),
        Err(e) => {
            debug!(
                agent_id,
                error = %crate::redact::scrub_urls(&e.to_string()),
                "ownerOf failed during proof verification"
            );
            None
        }
    };
    let wallet = identity
        .getAgentWallet(U256::from(agent_id))
        .call()
        .await
        .ok()
        .filter(|a| !a.is_zero());
    match (wallet, owner) {
        (None, None) => return Err(ProofRejection::RpcUnavailable),
        (w, o) => {
            let accepted = w == Some(payee) || o == Some(payee);
            if !accepted {
                return Err(ProofRejection::PayeeIsNotAgent);
            }
        }
    }

    Ok(())
}

/// Result of fetching and hashing the document behind `feedbackURI`.
enum AnchorFetch {
    NotDeclared,
    Unreachable,
    Mismatch,
    /// Fetched, and its keccak equals the declared `feedbackHash`.
    Matches(Vec<u8>),
}

/// Fetch the document the `feedbackHash` commits to and check that it hashes to
/// that value.
///
/// This is the half of the double copy that makes the anchor real: the chain
/// stores a hash, so if the URI is not resolvable the hash commits to a document
/// nobody can produce and the anchoring is decorative. Two numbers, not one --
/// "carries the tx anchored" and "the document is retrievable and its keccak
/// matches" -- and the second one measured 0,0% before Execution Market fixed
/// their CDN, for exactly this reason.
async fn fetch_and_hash_document(params: &FeedbackParams) -> AnchorFetch {
    let Some(expected) = params.feedback_hash else {
        return AnchorFetch::NotDeclared;
    };
    if params.feedback_uri.trim().is_empty() {
        return AnchorFetch::NotDeclared;
    }
    let Ok(url) = url::Url::parse(params.feedback_uri.trim()) else {
        // ipfs:// and friends are not fetchable here. Not a refusal: the anchor
        // is simply not auditable by us.
        return AnchorFetch::Unreachable;
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return AnchorFetch::Unreachable;
    }

    // SSRF-hardened fetch: the URI is attacker-supplied and we are inside a VPC
    // that can reach the instance metadata service. `safe_get` resolves, vets,
    // and pins the target, re-checking every redirect hop.
    let body = match crate::discovery_security::safe_get(
        "x402-facilitator-feedback-anchor/1.0",
        std::time::Duration::from_secs(10),
        &url,
    )
    .await
    {
        Ok(resp) if resp.status().is_success() => match resp.bytes().await {
            Ok(b) => b.to_vec(),
            Err(_) => return AnchorFetch::Unreachable,
        },
        _ => return AnchorFetch::Unreachable,
    };

    // Hash the exact bytes served. Re-serialising first would compare our
    // rendering of the document instead of the document, and the whole point is
    // that a third party can repeat this byte for byte.
    //
    // A 200 is not proof of a document, either: the same CDN misconfiguration
    // that served a 17.811-byte React page for every feedback URI answered 200
    // every time. The keccak is what tells the two apart.
    if keccak256(&body) != expected {
        return AnchorFetch::Mismatch;
    }
    AnchorFetch::Matches(body)
}

/// The document half, when there is a payment struct to cross-check against.
async fn evaluate_anchor(params: &FeedbackParams, proof: &ProofOfPayment) -> AnchorStatus {
    match fetch_and_hash_document(params).await {
        AnchorFetch::NotDeclared => AnchorStatus::NotDeclared,
        AnchorFetch::Unreachable => AnchorStatus::Unreachable,
        AnchorFetch::Mismatch => AnchorStatus::Mismatch,
        AnchorFetch::Matches(body) => {
            if document_mentions_payment(&body, &proof.transaction_hash) {
                AnchorStatus::Auditable
            } else {
                AnchorStatus::HashOnly
            }
        }
    }
}

/// The document half with no payment to cross-check: the hash still either
/// matches the served bytes or it does not.
async fn anchor_without_payment(params: &FeedbackParams) -> AnchorStatus {
    match fetch_and_hash_document(params).await {
        AnchorFetch::NotDeclared => AnchorStatus::NotDeclared,
        AnchorFetch::Unreachable => AnchorStatus::Unreachable,
        AnchorFetch::Mismatch => AnchorStatus::Mismatch,
        AnchorFetch::Matches(_) => AnchorStatus::HashOnly,
    }
}

/// Does the off-chain document name the payment the struct declares?
///
/// Structured lookup first, then a normalised scan of the whole document. The
/// scan is the fallback on purpose: the document schema is Execution Market's,
/// not ours, and a field rename on their side must not silently turn every
/// anchored rating into an unanchored one. Both sides are normalised to
/// lowercase without the `0x` prefix, because their DB stores hashes bare and
/// that mismatch has already produced one false negative.
fn document_mentions_payment(body: &[u8], tx: &TransactionHash) -> bool {
    let needle = normalize_hash(&tx.to_string());
    if needle.is_empty() {
        return false;
    }
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        const PATHS: &[&[&str]] = &[
            &["proof_of_payment", "transaction_hash"],
            &["proofOfPayment", "transactionHash"],
            &["payment_tx"],
            &["paymentTx"],
            &["payment", "transaction_hash"],
            &["payment", "transactionHash"],
            &["paymentInfo", "transactionHash"],
        ];
        for path in PATHS {
            let mut cursor = &json;
            let mut ok = true;
            for key in *path {
                match cursor.get(key) {
                    Some(next) => cursor = next,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                if let Some(found) = cursor.as_str() {
                    if normalize_hash(found) == needle {
                        return true;
                    }
                }
            }
        }
    }

    normalize_hash(text).contains(&needle)
}

/// Lowercase, without the `0x` prefix. Both sides, always.
fn normalize_hash(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase()
}

/// Anti-replay key for one (payment, agent) pair.
///
/// `#` separators rather than the `:` sketched in the handoff: the DynamoDB
/// store derives its `chain` attribute from everything before the first `#`
/// (`nonce_store.rs`), so `:` would file every row under a `chain` equal to the
/// entire key. Same semantics, same uniqueness, one fewer wart in the table.
pub fn proof_replay_key(network: &Network, tx: &TransactionHash, agent_id: &str) -> String {
    format!(
        "erc8004-proof#{}#{}#{}",
        network,
        normalize_hash(&tx.to_string()),
        agent_id
    )
}

fn evm_address(mixed: &MixedAddress) -> Result<Address, ProofRejection> {
    Address::try_from(mixed.clone()).map_err(|_| ProofRejection::NotEvmAddress)
}

/// Seconds since the epoch. Shared with the relay path so a deadline and a
/// freshness window are measured against the same clock.
pub fn unix_now_secs() -> u64 {
    unix_now()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TokenAmount;
    use alloy::providers::mock::Asserter;
    use alloy::providers::ProviderBuilder;
    use serde_json::json;

    const PAYER: [u8; 20] = [0x11; 20];
    const PAYEE: [u8; 20] = [0x22; 20];
    const TOKEN: [u8; 20] = [0x33; 20];
    const STRANGER: [u8; 20] = [0x44; 20];
    const IDENTITY: [u8; 20] = [0x80; 20];
    const AGENT_ID: u64 = 18896;
    const BLOCK: u64 = 123;

    fn addr(bytes: [u8; 20]) -> Address {
        Address::from(bytes)
    }

    fn mixed(bytes: [u8; 20]) -> MixedAddress {
        MixedAddress::Evm(addr(bytes).into())
    }

    /// A proof for a payment that really happened, as the settle path would
    /// have produced it.
    fn good_proof(now: u64) -> ProofOfPayment {
        ProofOfPayment::new(
            TransactionHash::Evm([0xAA; 32]),
            BLOCK,
            Network::Base,
            mixed(PAYER),
            mixed(PAYEE),
            TokenAmount::from(U256::from(1_000_000u64)),
            mixed(TOKEN),
            now,
        )
    }

    fn params_with(proof: ProofOfPayment, rater: Option<MixedAddress>) -> FeedbackParams {
        FeedbackParams {
            agent_id: json!(AGENT_ID),
            value: 87,
            value_decimals: 0,
            tag1: String::new(),
            tag2: String::new(),
            endpoint: String::new(),
            feedback_uri: String::new(),
            feedback_hash: None,
            score: None,
            proof: Some(proof),
            rater,
        }
    }

    fn word(bytes: [u8; 20]) -> String {
        format!("0x{}{}", "0".repeat(24), hex::encode(bytes))
    }

    /// `eth_getTransactionReceipt` for a successful payment.
    fn receipt_json(
        from: [u8; 20],
        to: [u8; 20],
        token: [u8; 20],
        amount: u64,
        block: u64,
    ) -> serde_json::Value {
        let transfer_sig = keccak256(b"Transfer(address,address,uint256)");
        json!({
            "transactionHash": format!("0x{}", hex::encode([0xAAu8; 32])),
            "transactionIndex": "0x0",
            "blockHash": format!("0x{}", hex::encode([0xBBu8; 32])),
            "blockNumber": format!("0x{:x}", block),
            "from": format!("0x{}", hex::encode(from)),
            "to": format!("0x{}", hex::encode(token)),
            "cumulativeGasUsed": "0x5208",
            "gasUsed": "0x5208",
            "contractAddress": null,
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "status": "0x1",
            "type": "0x2",
            "effectiveGasPrice": "0x1",
            "logs": [{
                "address": format!("0x{}", hex::encode(token)),
                "topics": [
                    format!("0x{}", hex::encode(transfer_sig)),
                    word(from),
                    word(to),
                ],
                "data": format!("0x{}", hex::encode(U256::from(amount).to_be_bytes::<32>())),
                "blockNumber": format!("0x{:x}", block),
                "transactionHash": format!("0x{}", hex::encode([0xAAu8; 32])),
                "transactionIndex": "0x0",
                "blockHash": format!("0x{}", hex::encode([0xBBu8; 32])),
                "logIndex": "0x0",
                "removed": false
            }]
        })
    }

    /// `eth_getBlockByNumber`, trimmed to what alloy needs to deserialise.
    fn block_json(number: u64, timestamp: u64) -> serde_json::Value {
        json!({
            "hash": format!("0x{}", hex::encode([0xBBu8; 32])),
            "parentHash": format!("0x{}", hex::encode([0xCCu8; 32])),
            "sha3Uncles": format!("0x{}", hex::encode([0u8; 32])),
            "miner": format!("0x{}", hex::encode([0u8; 20])),
            "stateRoot": format!("0x{}", hex::encode([0u8; 32])),
            "transactionsRoot": format!("0x{}", hex::encode([0u8; 32])),
            "receiptsRoot": format!("0x{}", hex::encode([0u8; 32])),
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "difficulty": "0x0",
            "number": format!("0x{:x}", number),
            "gasLimit": "0x1c9c380",
            "gasUsed": "0x5208",
            "timestamp": format!("0x{:x}", timestamp),
            "extraData": "0x",
            "mixHash": format!("0x{}", hex::encode([0u8; 32])),
            "nonce": "0x0000000000000000",
            "baseFeePerGas": "0x1",
            "totalDifficulty": "0x0",
            "size": "0x220",
            "transactions": [],
            "uncles": []
        })
    }

    fn now() -> u64 {
        unix_now()
    }

    /// Queue the happy path: receipt, block, `ownerOf`, `getAgentWallet`.
    fn asserter_for_happy_path(payee: [u8; 20], owner: [u8; 20], ts: u64) -> Asserter {
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, payee, TOKEN, 1_000_000, BLOCK));
        a.push_success(&block_json(BLOCK, ts));
        a.push_success(&word(owner));
        // getAgentWallet: zero, which is what every real agent we sampled on
        // Base answered.
        a.push_success(&word([0u8; 20]));
        a
    }

    async fn verdict(asserter: Asserter, params: &FeedbackParams) -> Result<(), ProofRejection> {
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        verify_proof_of_payment(
            &provider,
            addr(IDENTITY),
            Network::Base,
            AGENT_ID,
            params.rater.as_ref(),
            params.proof.as_ref().unwrap(),
        )
        .await
    }

    fn proof_with(tx: [u8; 32], amount: u64) -> ProofOfPayment {
        ProofOfPayment::new(
            TransactionHash::Evm(tx),
            123,
            Network::Base,
            MixedAddress::Evm(Address::from([0x11u8; 20]).into()),
            MixedAddress::Evm(Address::from([0x22u8; 20]).into()),
            TokenAmount::from(U256::from(amount)),
            MixedAddress::Evm(Address::from([0x33u8; 20]).into()),
            1_700_000_000,
        )
    }

    // ── verify_proof_of_payment: one test per way in ──────────────────────
    //
    // Each rejection gets its own case on purpose. A single "invalid proof"
    // test would pass while three of the checks silently did nothing.

    #[tokio::test]
    async fn a_real_payment_from_the_rater_to_the_agent_owner_verifies() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let asserter = asserter_for_happy_path(PAYEE, PAYEE, ts);
        assert_eq!(verdict(asserter, &params).await, Ok(()));
    }

    /// A payment on a cheap chain must not buy reputation on an expensive one.
    #[tokio::test]
    async fn a_payment_on_another_chain_is_refused() {
        let ts = now() - 60;
        let mut proof = good_proof(ts);
        proof.network = Network::Polygon;
        let params = params_with(proof, Some(mixed(PAYER)));
        assert_eq!(
            verdict(Asserter::new(), &params).await,
            Err(ProofRejection::NetworkMismatch)
        );
    }

    #[tokio::test]
    async fn a_proof_edited_after_the_settle_is_refused() {
        let ts = now() - 60;
        let mut proof = good_proof(ts);
        // Inflate the amount without recomputing the commitment.
        proof.amount = TokenAmount::from(U256::from(9_999_999u64));
        let params = params_with(proof, Some(mixed(PAYER)));
        assert_eq!(
            verdict(Asserter::new(), &params).await,
            Err(ProofRejection::PaymentHashMismatch)
        );
    }

    /// Without a rater, "the payer is the one rating" is not a question that has
    /// an answer -- and answering it optimistically is the whole bug.
    #[tokio::test]
    async fn a_request_that_does_not_say_who_is_rating_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), None);
        assert_eq!(
            verdict(Asserter::new(), &params).await,
            Err(ProofRejection::RaterMissing)
        );
    }

    #[tokio::test]
    async fn somebody_elses_payment_does_not_buy_a_rating() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(STRANGER)));
        assert_eq!(
            verdict(Asserter::new(), &params).await,
            Err(ProofRejection::PayerIsNotRater)
        );
    }

    #[tokio::test]
    async fn a_transaction_that_does_not_exist_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_success(&serde_json::Value::Null);
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::TransactionNotFound)
        );
    }

    #[tokio::test]
    async fn a_reverted_payment_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        let mut receipt = receipt_json(PAYER, PAYEE, TOKEN, 1_000_000, BLOCK);
        receipt["status"] = json!("0x0");
        a.push_success(&receipt);
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::TransactionReverted)
        );
    }

    #[tokio::test]
    async fn a_receipt_in_another_block_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, PAYEE, TOKEN, 1_000_000, BLOCK + 1));
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::BlockNumberMismatch)
        );
    }

    /// The declared timestamp is not covered by `paymentHash`, so freshness is
    /// only meaningful when it is read from the block itself.
    #[tokio::test]
    async fn a_timestamp_the_chain_does_not_agree_with_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, PAYEE, TOKEN, 1_000_000, BLOCK));
        // The block says something else -- e.g. a stale proof rewritten to look
        // fresh.
        a.push_success(&block_json(BLOCK, ts - 999));
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::TimestampMismatch)
        );
    }

    #[tokio::test]
    async fn a_payment_older_than_the_window_is_refused() {
        let old = now() - (30 * 24 * 3600);
        let params = params_with(good_proof(old), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, PAYEE, TOKEN, 1_000_000, BLOCK));
        a.push_success(&block_json(BLOCK, old));
        assert_eq!(verdict(a, &params).await, Err(ProofRejection::Expired));
    }

    /// The amount is read from the `Transfer` log, not from the struct: a proof
    /// claiming a payment that the transaction does not contain is refused.
    #[tokio::test]
    async fn an_amount_the_transaction_does_not_contain_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        // Same parties, one unit short.
        a.push_success(&receipt_json(PAYER, PAYEE, TOKEN, 999_999, BLOCK));
        a.push_success(&block_json(BLOCK, ts));
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::TransferNotFound)
        );
    }

    /// A transfer of the right amount in the WRONG token does not count either.
    #[tokio::test]
    async fn a_transfer_of_another_token_is_refused() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, PAYEE, STRANGER, 1_000_000, BLOCK));
        a.push_success(&block_json(BLOCK, ts));
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::TransferNotFound)
        );
    }

    /// Paying somebody who is not this agent buys nothing.
    #[tokio::test]
    async fn a_payment_to_a_stranger_is_refused() {
        let ts = now() - 60;
        let mut proof = good_proof(ts);
        proof.payee = mixed(STRANGER);
        let proof = ProofOfPayment::new(
            proof.transaction_hash,
            proof.block_number,
            proof.network,
            proof.payer,
            proof.payee,
            proof.amount,
            proof.token,
            proof.timestamp,
        );
        let params = params_with(proof, Some(mixed(PAYER)));
        // The transfer really did go to the stranger, and the agent's owner is
        // somebody else.
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, STRANGER, TOKEN, 1_000_000, BLOCK));
        a.push_success(&block_json(BLOCK, ts));
        a.push_success(&word(PAYEE));
        a.push_success(&word([0u8; 20]));
        assert_eq!(
            verdict(a, &params).await,
            Err(ProofRejection::PayeeIsNotAgent)
        );
    }

    /// `getAgentWallet` is zero for most real agents, so `ownerOf` is
    /// load-bearing -- but where a wallet IS set, paying it must also work.
    #[tokio::test]
    async fn paying_the_declared_agent_wallet_verifies_even_when_the_owner_differs() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_success(&receipt_json(PAYER, PAYEE, TOKEN, 1_000_000, BLOCK));
        a.push_success(&block_json(BLOCK, ts));
        // The NFT owner is a stranger...
        a.push_success(&word(STRANGER));
        // ...but the agent declared PAYEE as its payment wallet.
        a.push_success(&word(PAYEE));
        assert_eq!(verdict(a, &params).await, Ok(()));
    }

    /// An unreachable chain produces "no verdict", never "not paid".
    #[tokio::test]
    async fn an_rpc_failure_is_no_verdict_rather_than_a_refusal() {
        let ts = now() - 60;
        let params = params_with(good_proof(ts), Some(mixed(PAYER)));
        let a = Asserter::new();
        a.push_failure_msg("connection reset");
        let out = verdict(a, &params).await;
        assert_eq!(out, Err(ProofRejection::RpcUnavailable));
        assert!(out.unwrap_err().is_retryable());
    }

    /// One payment, one rating for a given agent.
    ///
    /// The claim is what stops a single settle from being cashed in fifty
    /// times; the same payment rating a DIFFERENT agent is a different key and
    /// is stopped instead by `PayeeIsNotAgent`.
    #[tokio::test]
    async fn the_same_payment_cannot_buy_two_ratings_for_the_same_agent() {
        use crate::nonce_store::{MemoryNonceStore, NonceStore, NonceStoreError};

        let store = MemoryNonceStore::new();
        let tx = TransactionHash::Evm([0xAA; 32]);
        let key = proof_replay_key(&Network::Base, &tx, "18896");

        assert!(store
            .check_and_mark_used(&key, replay_ttl_secs())
            .await
            .is_ok());
        assert!(matches!(
            store.check_and_mark_used(&key, replay_ttl_secs()).await,
            Err(NonceStoreError::NonceAlreadyUsed(_))
        ));

        // A different agent is a different key -- the pair is what is spent,
        // not the payment alone.
        let other = proof_replay_key(&Network::Base, &tx, "58517");
        assert_ne!(key, other);
        assert!(store
            .check_and_mark_used(&other, replay_ttl_secs())
            .await
            .is_ok());
    }

    #[test]
    fn the_replay_key_is_normalised_on_both_sides() {
        let key = proof_replay_key(&Network::Base, &TransactionHash::Evm([0xAB; 32]), "18896");
        assert!(key.starts_with("erc8004-proof#base#"));
        assert!(!key.contains("0x"), "prefix leaked into the key: {key}");
        assert!(!key.contains("AB"), "case leaked into the key: {key}");
        assert!(key.ends_with("#18896"));
    }

    /// The DynamoDB store slices `chain` off the front of the key. If the
    /// separator ever goes back to `:` every row files itself under a `chain`
    /// equal to the whole key.
    #[test]
    fn the_replay_key_keeps_the_stores_chain_prefix_convention() {
        let key = proof_replay_key(&Network::Base, &TransactionHash::Evm([0x01; 32]), "1");
        assert_eq!(key.split('#').next(), Some("erc8004-proof"));
    }

    #[test]
    fn a_tampered_proof_fails_its_own_hash() {
        let mut proof = proof_with([0x01; 32], 1000);
        assert_eq!(proof.recompute_payment_hash(), proof.payment_hash);
        // Somebody inflates the amount after the settle produced the struct.
        proof.amount = TokenAmount::from(U256::from(9_999_999u64));
        assert_ne!(proof.recompute_payment_hash(), proof.payment_hash);
    }

    #[test]
    fn normalising_a_hash_strips_prefix_and_case() {
        assert_eq!(normalize_hash("0xAbCd"), "abcd");
        assert_eq!(normalize_hash("abcd"), "abcd");
        assert_eq!(normalize_hash("  0XABCD "), "abcd");
    }

    #[test]
    fn a_document_naming_the_payment_in_their_schema_counts() {
        let doc = br#"{"feedback":{"value":87},"proof_of_payment":{"transaction_hash":"a1b2c3"}}"#;
        let tx = TransactionHash::Evm({
            let mut b = [0u8; 32];
            b[0] = 0xa1;
            b[1] = 0xb2;
            b[2] = 0xc3;
            b
        });
        // Their DB stores the hash bare; ours renders it 0x-prefixed. The
        // comparison has to survive that, which is the false negative they
        // already paid for once.
        let full = normalize_hash(&tx.to_string());
        let doc_json = format!(r#"{{"proof_of_payment":{{"transaction_hash":"{full}"}}}}"#);
        assert!(document_mentions_payment(doc_json.as_bytes(), &tx));
        // A document about some other payment does not count.
        assert!(!document_mentions_payment(doc, &tx));
    }

    #[test]
    fn a_document_that_mentions_the_payment_anywhere_still_counts() {
        let tx = TransactionHash::Evm([0x7f; 32]);
        let bare = normalize_hash(&tx.to_string());
        let doc = format!(r#"{{"unexpected_field_name":"0x{bare}"}}"#);
        assert!(document_mentions_payment(doc.as_bytes(), &tx));
    }

    #[test]
    fn the_gate_defaults_to_measuring_not_rejecting() {
        std::env::remove_var(ENV_REQUIRE_PROOF);
        assert!(!is_proof_required());
        std::env::set_var(ENV_REQUIRE_PROOF, "true");
        assert!(is_proof_required());
        std::env::set_var(ENV_REQUIRE_PROOF, "false");
        assert!(!is_proof_required());
        std::env::remove_var(ENV_REQUIRE_PROOF);
    }

    #[test]
    fn the_freshness_window_defaults_to_seven_days() {
        std::env::remove_var(ENV_PROOF_MAX_AGE_SECS);
        assert_eq!(proof_max_age_secs(), 7 * 24 * 3600);
        std::env::set_var(ENV_PROOF_MAX_AGE_SECS, "60");
        assert_eq!(proof_max_age_secs(), 60);
        // A zero or garbage value must not disable freshness silently.
        std::env::set_var(ENV_PROOF_MAX_AGE_SECS, "0");
        assert_eq!(proof_max_age_secs(), 7 * 24 * 3600);
        std::env::set_var(ENV_PROOF_MAX_AGE_SECS, "not-a-number");
        assert_eq!(proof_max_age_secs(), 7 * 24 * 3600);
        std::env::remove_var(ENV_PROOF_MAX_AGE_SECS);
    }

    /// An outage is not a verdict. Rejecting on `RpcUnavailable` would let our
    /// own downtime erase somebody's reputation claim.
    #[test]
    fn an_unreachable_rpc_never_blocks_the_write() {
        let report = ProofReport {
            rejection: Some(ProofRejection::RpcUnavailable),
            anchor: AnchorStatus::NotDeclared,
            enforced: true,
        };
        assert!(!report.should_reject());
        assert!(!report.is_verified());
    }

    #[test]
    fn phase_one_verifies_without_rejecting() {
        let report = ProofReport {
            rejection: Some(ProofRejection::PayerIsNotRater),
            anchor: AnchorStatus::NotDeclared,
            enforced: false,
        };
        assert!(!report.should_reject());
        assert!(!report.is_verified());

        let enforced = ProofReport {
            enforced: true,
            ..report.clone()
        };
        assert!(enforced.should_reject());
    }

    #[test]
    fn every_rejection_has_a_distinct_stable_token() {
        let all = [
            ProofRejection::Missing,
            ProofRejection::RaterMissing,
            ProofRejection::NetworkMismatch,
            ProofRejection::UnverifiableChain,
            ProofRejection::NotEvmTransaction,
            ProofRejection::NotEvmAddress,
            ProofRejection::TransactionNotFound,
            ProofRejection::TransactionReverted,
            ProofRejection::BlockNumberMismatch,
            ProofRejection::TimestampMismatch,
            ProofRejection::TransferNotFound,
            ProofRejection::PayerIsNotRater,
            ProofRejection::PayeeIsNotAgent,
            ProofRejection::Expired,
            ProofRejection::PaymentHashMismatch,
            ProofRejection::Replayed,
            ProofRejection::DocumentHashMismatch,
            ProofRejection::DocumentPaymentMismatch,
            ProofRejection::RpcUnavailable,
        ];
        let mut seen = std::collections::HashSet::new();
        for r in all {
            assert!(seen.insert(r.as_str()), "duplicate token: {}", r.as_str());
            // The token reaches clients and logs; it must never carry an
            // address or a URL.
            assert!(!r.as_str().contains("0x"));
            assert!(!r.as_str().contains("://"));
        }
        assert_eq!(seen.len(), 19);
    }

    #[test]
    fn only_no_verdict_is_retryable() {
        assert!(ProofRejection::RpcUnavailable.is_retryable());
        for r in [
            ProofRejection::Missing,
            ProofRejection::PayerIsNotRater,
            ProofRejection::Replayed,
            ProofRejection::Expired,
        ] {
            assert!(!r.is_retryable(), "{} must not be retryable", r.as_str());
        }
    }

    /// Exactly two reasons are allowed not to block a write, and they are the
    /// two that mean "not decided" rather than "refused".
    #[test]
    fn only_no_verdict_and_a_chain_we_cannot_read_let_the_write_through() {
        assert!(!ProofRejection::RpcUnavailable.blocks_write());
        assert!(!ProofRejection::UnverifiableChain.blocks_write());
        for r in [
            ProofRejection::Missing,
            ProofRejection::RaterMissing,
            ProofRejection::NetworkMismatch,
            // A proof whose tx hash is not an EVM hash IS a refusal: it must
            // not be able to borrow the SVM exemption to slip past the gate.
            ProofRejection::NotEvmTransaction,
            ProofRejection::NotEvmAddress,
            ProofRejection::TransactionNotFound,
            ProofRejection::TransactionReverted,
            ProofRejection::BlockNumberMismatch,
            ProofRejection::TimestampMismatch,
            ProofRejection::TransferNotFound,
            ProofRejection::PayerIsNotRater,
            ProofRejection::PayeeIsNotAgent,
            ProofRejection::Expired,
            ProofRejection::PaymentHashMismatch,
            ProofRejection::Replayed,
            ProofRejection::DocumentHashMismatch,
            ProofRejection::DocumentPaymentMismatch,
        ] {
            assert!(r.blocks_write(), "{} must block the write", r.as_str());
        }
    }

    /// A Solana feedback is never refused by this gate, enforced or not.
    #[tokio::test]
    async fn the_svm_path_records_the_gap_without_blocking() {
        std::env::set_var(ENV_REQUIRE_PROOF, "true");
        let params = FeedbackParams {
            agent_id: serde_json::json!("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv"),
            value: 87,
            value_decimals: 0,
            tag1: String::new(),
            tag2: String::new(),
            endpoint: String::new(),
            feedback_uri: String::new(),
            feedback_hash: None,
            score: Some(95),
            proof: Some(proof_with([0x01; 32], 1000)),
            rater: None,
        };
        let report = evaluate_svm_feedback_proof(&params).await;
        std::env::remove_var(ENV_REQUIRE_PROOF);

        assert_eq!(report.rejection, Some(ProofRejection::UnverifiableChain));
        assert!(report.enforced);
        assert!(
            !report.should_reject(),
            "enforcing a check that never ran would silently disable Solana reputation"
        );
    }
}
