//! What actually went wrong when a chain write failed, as a type.
//!
//! # Why this exists
//!
//! Until 2026-09-10 the settle paths asked one question — "is this the node's
//! fault or the caller's?" — and answered it with a boolean over JSON-RPC error
//! codes. `-32000` meant "the node could not answer", so the caller got 502 and
//! `Retry-After: 30`.
//!
//! `-32000` is also what geth answers when OUR OWN signer cannot pay for gas:
//!
//! ```text
//! insufficient funds for gas * price + value: balance 82861633384675957709,
//! queued cost 82799377752610042973, tx cost 78463640160630732,
//! overshot 16208008094715996
//! ```
//!
//! Over the 24 hours to 2026-09-10 that single condition produced 7,196 of the
//! error lines on this service — every one of them reported to the caller as an
//! upstream outage with an invitation to come back in thirty seconds, for a
//! condition no caller can influence and no retry can improve. All of them
//! carried the same balance, so they were one signer on one network: the EVM
//! mainnet facilitator wallet on Polygon, with 400 transactions queued
//! (`eth_getTransactionCount` latest 1157, pending 1557) holding 82.7994 of its
//! 82.8616 POL. Usable margin 0.0623 POL against a transaction that costs
//! 0.0785.
//!
//! # The shape of the answer
//!
//! One boolean cannot carry that, so this module classifies by **stage** (where
//! in the write the failure happened) and **reason** (what happened), and each
//! pair carries its own status code, its own bounded category token and its own
//! retry advice — including *no* retry advice, which is the correct answer for
//! a transaction that may already be in flight.
//!
//! Two rules are load-bearing:
//!
//! * **A revert wins over everything.** If the chain executed the call and
//!   rejected it, that is an answer, not an outage — even when the revert
//!   reason happens to contain a word that appears in one of the lists below.
//! * **We never advertise a retry for a transaction that may have been
//!   broadcast.** [`Reason::BroadcastUncertain`] and [`Reason::ReceiptPending`]
//!   carry no `Retry-After` at all, because a retry there is a second payment.

use std::fmt;

/// Where in the write the failure happened.
///
/// The stage decides whether a retry could even be safe, independently of the
/// reason: nothing that got past [`Stage::Broadcast`] can be retried blindly,
/// however benign the reason looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Before anything was handed to the node: payload validation, or a
    /// simulation the chain executed and rejected.
    Request,
    /// While handing the transaction to the node. The node may or may not have
    /// queued it, depending on the reason.
    Broadcast,
    /// The node accepted the transaction; we are waiting for a verdict.
    Confirmation,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Request => "request",
            Stage::Broadcast => "broadcast",
            Stage::Confirmation => "confirmation",
        }
    }
}

/// Why the write failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The chain ran the call and rejected it. Bad signature, expired
    /// authorization, a custom Solidity error. The caller can act on it.
    PayloadRejected,
    /// The facilitator's own signer cannot cover `gas * price + value` on this
    /// network. Nothing about the request is wrong and no retry can help until
    /// an operator drains the queue or funds the wallet.
    SignerUnfunded,
    /// The node refused on nonce or mempool grounds. The transaction provably
    /// never queued, so a retry lands on a clean slot.
    NonceOrMempool,
    /// The node refused because we are asking too often. A real rate limit,
    /// matched on phrasing and on `-32005` — never on a bare `429`, which also
    /// appears inside wei amounts.
    RateLimited,
    /// The node could not answer at all: pruned history, missing headers,
    /// retries exhausted, transport failure.
    Transport,
    /// We handed the transaction over and never learned its fate. It may be
    /// mined. A retry is a second payment.
    BroadcastUncertain,
    /// Broadcast succeeded; the receipt has not arrived yet. Same rule: not a
    /// retry, a lookup.
    ReceiptPending,
    /// Nothing recognised it. Stays a caller error, which is the conservative
    /// answer: a wrong 502 tells someone with a genuinely broken payload to sit
    /// and wait for us.
    Unclassified,
}

/// A classified chain-write failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainFailure {
    pub stage: Stage,
    pub reason: Reason,
}

impl fmt::Display for ChainFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.stage.as_str(), self.category())
    }
}

/// How long to tell a caller to wait for a gas shortfall.
///
/// Five minutes, not thirty seconds. The condition changes when an operator
/// drains a stuck queue or funds the wallet, which is a human timescale; a
/// shorter hint is an invitation to hammer a signer that cannot move. The
/// measured incident ran for at least 24 hours.
const UNFUNDED_RETRY_AFTER_SECS: u32 = 300;

/// Spread for [`UNFUNDED_RETRY_AFTER_SECS`], as a fraction of it.
///
/// Every caller that hits an unfunded signer hits it at once, and a fixed hint
/// makes them all come back at once too. The jitter is deterministic per
/// response rather than random so the value stays reproducible in a test.
const UNFUNDED_RETRY_JITTER: u32 = UNFUNDED_RETRY_AFTER_SECS / 10;

impl ChainFailure {
    /// Bounded token for logs, `/events` and the transaction index.
    ///
    /// Closed set, deliberately: the raw error carries addresses and, on a bad
    /// day, an RPC URL with the API key inside it (`src/redact.rs` exists
    /// because exactly that leaked once).
    pub fn category(&self) -> &'static str {
        match self.reason {
            Reason::PayloadRejected => "payload_rejected",
            Reason::SignerUnfunded => "facilitator_signer_unfunded",
            Reason::NonceOrMempool => "upstream_nonce_or_mempool",
            Reason::RateLimited => "upstream_rate_limited",
            Reason::Transport => "upstream_rpc_unavailable",
            Reason::BroadcastUncertain => "broadcast_uncertain",
            Reason::ReceiptPending => "receipt_pending",
            Reason::Unclassified => "unclassified",
        }
    }

    /// HTTP status this failure deserves, as a bare `u16` so this module does
    /// not depend on the HTTP layer.
    ///
    /// `503` for the two conditions that are OURS — an unfunded signer and a
    /// rate limit we provoked — because `502` claims an upstream fault and
    /// `400` blames the caller. Both are wrong in a way that costs someone
    /// hours of debugging a payload that was fine.
    pub fn http_status(&self) -> u16 {
        match self.reason {
            Reason::PayloadRejected | Reason::Unclassified => 400,
            Reason::SignerUnfunded | Reason::RateLimited => 503,
            Reason::NonceOrMempool
            | Reason::Transport
            | Reason::BroadcastUncertain
            | Reason::ReceiptPending => 502,
        }
    }

    /// Whether a caller may safely repeat the request.
    ///
    /// `false` for anything past broadcast. This is the whole no-double-spend
    /// rule in one predicate, and [`Self::retry_after_secs`] is derived from
    /// it rather than decided separately, so the two can never disagree.
    pub fn retryable(&self) -> bool {
        match self.reason {
            Reason::SignerUnfunded
            | Reason::NonceOrMempool
            | Reason::RateLimited
            | Reason::Transport => true,
            Reason::PayloadRejected
            | Reason::BroadcastUncertain
            | Reason::ReceiptPending
            | Reason::Unclassified => false,
        }
    }

    /// `Retry-After`, in seconds, or `None` when no retry is advised.
    ///
    /// `salt` spreads the gas-shortfall hint so a fleet of callers stalled on
    /// the same signer does not come back in lockstep. Pass anything that
    /// varies per response; the caller-visible effect is bounded to ±10%.
    pub fn retry_after_secs(&self, salt: u64) -> Option<u32> {
        if !self.retryable() {
            return None;
        }
        match self.reason {
            // The condition is a human one. Do not invite a fast retry.
            Reason::SignerUnfunded => {
                let spread = 2 * UNFUNDED_RETRY_JITTER + 1;
                let offset = (salt % u64::from(spread)) as u32;
                Some(UNFUNDED_RETRY_AFTER_SECS - UNFUNDED_RETRY_JITTER + offset)
            }
            // Ours to fix, but it clears on its own as the window rolls.
            Reason::RateLimited => Some(60),
            // Unchanged from the behaviour this replaced.
            Reason::NonceOrMempool | Reason::Transport => Some(30),
            _ => None,
        }
    }

    /// One sentence for the caller. Bounded text: no addresses, no amounts, no
    /// RPC URLs.
    pub fn client_message(&self) -> &'static str {
        match self.reason {
            Reason::PayloadRejected => {
                "The chain executed this call and rejected it. The request is what needs fixing."
            }
            Reason::SignerUnfunded => {
                "The facilitator's signer for this network cannot currently cover gas. \
                 The request was not rejected and nothing about it needs changing; \
                 an operator has to restore the signer's usable margin. Retry later."
            }
            Reason::NonceOrMempool => {
                "The node refused this transaction on nonce or mempool grounds and never \
                 queued it. Retry later."
            }
            Reason::RateLimited => {
                "The facilitator is being rate limited by this network's RPC provider. \
                 The request was not rejected. Retry later."
            }
            Reason::Transport => {
                "Upstream RPC unavailable for this network; the request was not rejected, \
                 the node could not answer. Retry later."
            }
            Reason::BroadcastUncertain => {
                "The transaction was handed to the network and no verdict was reached. \
                 It may be mined. Do NOT retry: check the chain first."
            }
            Reason::ReceiptPending => {
                "The transaction was broadcast and its receipt has not arrived. \
                 Do NOT retry: check the chain first."
            }
            Reason::Unclassified => {
                "The call failed for a reason the facilitator could not classify."
            }
        }
    }

    /// Classify a `{:?}`-rendered chain error.
    ///
    /// The input is the debug rendering because the handlers are generic over
    /// `A::Error` and the concrete enum is not in scope for them; the variant
    /// identifier that `{:?}` puts first is more stable than the prose anyway.
    pub fn classify(debug: &str) -> Self {
        let lower = debug.to_ascii_lowercase();

        // 1. The chain answered. Nothing below may reinterpret that, however
        //    the revert reason happens to be worded.
        if lower.contains("execution reverted") || lower.contains(" reverted on ") {
            return Self {
                stage: Stage::Request,
                reason: Reason::PayloadRejected,
            };
        }

        // 2. Anything that may already be on the wire, before any reason that
        //    would invite a retry.
        if is_unconfirmed_broadcast(&lower) {
            return Self {
                stage: Stage::Broadcast,
                reason: Reason::BroadcastUncertain,
            };
        }
        if is_receipt_pending(&lower) {
            return Self {
                stage: Stage::Confirmation,
                reason: Reason::ReceiptPending,
            };
        }

        // 3. Our own gas. This is the one the old boolean called an outage.
        if is_signer_unfunded(&lower) {
            return Self {
                stage: Stage::Broadcast,
                reason: Reason::SignerUnfunded,
            };
        }

        if is_nonce_or_mempool(&lower) {
            return Self {
                stage: Stage::Broadcast,
                reason: Reason::NonceOrMempool,
            };
        }

        if is_rate_limited(&lower) {
            return Self {
                stage: Stage::Request,
                reason: Reason::RateLimited,
            };
        }

        if is_transport(&lower) {
            return Self {
                stage: Stage::Request,
                reason: Reason::Transport,
            };
        }

        Self {
            stage: Stage::Request,
            reason: Reason::Unclassified,
        }
    }
}

/// Whether the transaction may already be in the network's hands.
///
/// Matches what `chain/evm.rs` and the escrow path actually emit: the
/// `SettlementUnconfirmed` variant name (the escrow path flattens every
/// `FacilitatorLocalError` into a string, so the variant is all that survives),
/// and the two prose forms the nonce-retry guard produces when it declines to
/// retry.
fn is_unconfirmed_broadcast(lower: &str) -> bool {
    lower.contains("settlementunconfirmed")
        || lower.contains("settlement_unconfirmed")
        || lower.contains("may have been mined")
}

/// Broadcast succeeded and the receipt has not arrived.
fn is_receipt_pending(lower: &str) -> bool {
    lower.contains("receipt never arrived") || lower.contains("timed out waiting for receipt")
}

/// Whether the node refused because OUR signer cannot pay for the transaction.
///
/// `insufficient funds` with a space is specific: geth, erigon, nethermind and
/// reth all use it for the sender's native balance and nothing else. The
/// facilitator's own typed variant for a PAYER who is short renders as
/// `InsufficientFunds(..)` with no space, so the two cannot be confused — and
/// the revert check in [`ChainFailure::classify`] runs first anyway, which
/// covers a token contract that words its revert this way.
fn is_signer_unfunded(lower: &str) -> bool {
    lower.contains("insufficient funds")
}

/// Nonce and mempool refusals. The transaction never entered the pool.
///
/// Delegates to `chain/evm.rs` rather than keeping a second string list: that
/// module decides on the same phrasings whether the reserved nonce may be
/// handed back, and the two answers must never drift apart. A retry we advise
/// for a nonce the allocator did NOT release widens a gap instead of curing it.
fn is_nonce_or_mempool(lower: &str) -> bool {
    crate::chain::evm::is_mempool_full(lower) || crate::chain::evm::is_nonce_error(lower)
}

/// A real rate limit.
///
/// Deliberately does NOT match a bare `429`. An early count of that substring
/// during the 2026-09-09 audit matched the digits inside wei balances and
/// produced a rate-limiting incident that did not exist.
fn is_rate_limited(lower: &str) -> bool {
    lower.contains("too many requests")
        || lower.contains("rate limit")
        || lower.contains("rate-limit")
        || lower.contains("-32005")
        || lower.contains("exceeded the quota")
        || lower.contains("request limit reached")
        || lower.contains("capacity exceeded")
}

/// The node could not answer at all.
///
/// The same three JSON-RPC codes the previous boolean used, plus the transport
/// shapes. Unchanged on purpose: this is the one class whose behaviour the A1
/// fix does not alter.
fn is_transport(lower: &str) -> bool {
    const NODE_CODES: [&str; 3] = ["-32000", "-32603", "-32801"];
    NODE_CODES.iter().any(|c| lower.contains(c))
        || lower.contains("max retries exceeded")
        || lower.contains("transport(")
}

/// The numbers a node puts in a gas-shortfall message.
///
/// Parsed so the operator sees the thing that actually matters — the margin
/// left AFTER the queue — instead of a total balance that looks healthy. In the
/// measured incident the wallet held 82.86 POL and could not pay for a 0.078
/// POL transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GasShortfall {
    /// The signer's total native balance, in wei.
    pub balance: Option<u128>,
    /// What transactions already queued from this signer have committed.
    pub queued_cost: Option<u128>,
    /// What the refused transaction needed.
    pub tx_cost: Option<u128>,
    /// How much it missed by.
    pub overshot: Option<u128>,
}

impl GasShortfall {
    /// Pull the figures out of a node's message, if it carries any.
    ///
    /// Handles both shapes seen in production: geth's txpool form
    /// (`balance/queued cost/tx cost/overshot`) and its plainer
    /// `have N want M` form.
    pub fn parse(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        Self {
            balance: number_after(&lower, "balance ").or_else(|| number_after(&lower, "have ")),
            queued_cost: number_after(&lower, "queued cost "),
            tx_cost: number_after(&lower, "tx cost ").or_else(|| number_after(&lower, "want ")),
            overshot: number_after(&lower, "overshot "),
        }
    }

    /// Balance minus what the queue has already committed: the margin the next
    /// transaction can actually draw on.
    ///
    /// `None` when the message did not carry both figures. Saturating rather
    /// than wrapping — a node that reports a queue larger than the balance is
    /// describing zero usable margin, not an enormous one.
    pub fn usable(&self) -> Option<u128> {
        match (self.balance, self.queued_cost) {
            (Some(b), Some(q)) => Some(b.saturating_sub(q)),
            (Some(b), None) => Some(b),
            _ => None,
        }
    }

    /// Whether anything at all was recovered.
    pub fn is_empty(&self) -> bool {
        self.balance.is_none()
            && self.queued_cost.is_none()
            && self.tx_cost.is_none()
            && self.overshot.is_none()
    }
}

/// First run of decimal digits following `needle`, as a `u128`.
///
/// Wei amounts overflow `u64` routinely, and a value too large even for `u128`
/// yields `None` rather than a wrong number.
fn number_after(haystack: &str, needle: &str) -> Option<u128> {
    let start = haystack.find(needle)? + needle.len();
    let digits: String = haystack[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message that produced 7,196 error lines in 24 hours, sanitized only
    /// by dropping the surrounding tracing frame. This is the fixture the whole
    /// change exists for: before it, this string was reported to the caller as
    /// `upstream_rpc_unavailable` with `Retry-After: 30`.
    const REAL_UNFUNDED: &str = r#"ContractCall("ErrorResp(ErrorPayload { code: -32000, message: \"insufficient funds for gas * price + value: balance 82861633384675957709, queued cost 82799377752610042973, tx cost 78463640160630732, overshot 16208008094715996\", data: None })")"#;

    /// The other phrasing the same condition takes, with the address sanitized.
    const REAL_UNFUNDED_HAVE_WANT: &str = r#"ErrorResp(ErrorPayload { code: -32000, message: "insufficient funds for gas * price + value: address 0x0000000000000000000000000000000000000001 have 62255632065914740 want 78463640160630732", data: None })"#;

    fn classify(s: &str) -> ChainFailure {
        ChainFailure::classify(s)
    }

    /// The headline: a signer that cannot pay for gas is not an upstream
    /// outage, and it is not a malformed request either.
    #[test]
    fn a_gas_shortfall_is_named_as_one() {
        for fixture in [REAL_UNFUNDED, REAL_UNFUNDED_HAVE_WANT] {
            let f = classify(fixture);
            assert_eq!(f.reason, Reason::SignerUnfunded, "{fixture}");
            assert_eq!(f.stage, Stage::Broadcast);
            assert_eq!(f.category(), "facilitator_signer_unfunded");
            assert_eq!(
                f.http_status(),
                503,
                "502 claims an upstream fault; 400 blames the caller. Both are wrong here"
            );
        }
    }

    /// The `-32000` in that message is exactly why the old boolean got it
    /// wrong. Guard the precedence explicitly so a future edit that reorders
    /// the checks fails here rather than in production.
    #[test]
    fn the_node_code_does_not_win_over_the_gas_reason() {
        assert!(REAL_UNFUNDED.contains("-32000"));
        assert_eq!(classify(REAL_UNFUNDED).reason, Reason::SignerUnfunded);
    }

    /// "Cero reintentos agresivos mientras no cambie la condicion": the hint
    /// stays in minutes, jittered, never in the thirty seconds a transport
    /// blip gets.
    #[test]
    fn a_gas_shortfall_is_never_a_thirty_second_retry() {
        for salt in [0u64, 1, 7, 999, u64::MAX] {
            let after = classify(REAL_UNFUNDED)
                .retry_after_secs(salt)
                .expect("a shortfall is retryable, just not soon");
            assert!(
                (270..=330).contains(&after),
                "{after}s is outside the bounded 300s ±10% window"
            );
        }
    }

    /// The jitter has to actually spread, or a fleet of stalled callers comes
    /// back in lockstep.
    #[test]
    fn the_retry_hint_is_spread_across_callers() {
        let f = classify(REAL_UNFUNDED);
        let values: std::collections::BTreeSet<u32> =
            (0..200u64).filter_map(|s| f.retry_after_secs(s)).collect();
        assert!(
            values.len() > 10,
            "only {} distinct hints; callers would synchronize",
            values.len()
        );
    }

    /// A transaction that may be on the wire must never come back with retry
    /// advice attached. This is the no-double-broadcast rule.
    #[test]
    fn nothing_past_broadcast_is_ever_advertised_as_retryable() {
        for fixture in [
            r#"ContractCall("SettlementUnconfirmed(Evm(0x1111111111111111111111111111111111111111111111111111111111111111), Polygon) (operator=0x0000000000000000000000000000000000000002, selector=0x1a2b3c4d)")"#,
            "Nonce error and TX count could not be verified, original TX may have been mined: ErrorResp(ErrorPayload { code: -32000, message: \"nonce too low\" })",
        ] {
            let f = classify(fixture);
            assert_eq!(f.reason, Reason::BroadcastUncertain, "{fixture}");
            assert!(!f.retryable());
            assert_eq!(f.retry_after_secs(0), None, "{fixture}");
        }

        // A receipt that never arrived is the same rule one stage later. On
        // EVM this shape reaches the caller as `SettlementUnconfirmed`, which
        // carries the hash; the reason exists for the chains and phrasings
        // that report an outstanding receipt without one.
        let pending = classify("timed out waiting for receipt");
        assert_eq!(pending.reason, Reason::ReceiptPending);
        assert_eq!(pending.stage, Stage::Confirmation);
        assert!(!pending.retryable());
        assert_eq!(pending.retry_after_secs(0), None);
    }

    /// A gas shortfall provably never queued the transaction, which is what
    /// makes it safe to invite a retry at all. `chain/evm.rs` hands the nonce
    /// back on the same condition; if that ever stops being true, the retry
    /// this module advertises becomes a nonce gap.
    #[test]
    fn the_only_retry_we_advise_after_a_shortfall_lands_on_a_released_nonce() {
        for fixture in [REAL_UNFUNDED, REAL_UNFUNDED_HAVE_WANT] {
            assert!(
                crate::chain::evm::is_pre_broadcast_rejection(fixture),
                "advising a retry for a transaction that may be queued is a second broadcast"
            );
        }
    }

    /// The audit's false positive, pinned. `429` appears inside wei amounts;
    /// counting the bare substring invented a rate-limiting incident that did
    /// not exist.
    #[test]
    fn digits_inside_a_balance_are_not_a_rate_limit() {
        let looks_like_429 = r#"ErrorResp(ErrorPayload { code: -32000, message: "insufficient funds for gas * price + value: balance 4291633384675957709, queued cost 4290377752610042973, tx cost 78463640160630732, overshot 429", data: None })"#;
        assert_eq!(classify(looks_like_429).reason, Reason::SignerUnfunded);
    }

    /// ...while a real one is still recognised.
    #[test]
    fn a_real_rate_limit_is_recognised() {
        for fixture in [
            r#"Transport(Custom("HTTP status client error (429 Too Many Requests) for url"))"#,
            r#"ErrorResp(ErrorPayload { code: -32005, message: "rate limit exceeded", data: None })"#,
            "your app has exceeded the quota for this month",
        ] {
            let f = classify(fixture);
            assert_eq!(f.reason, Reason::RateLimited, "{fixture}");
            assert_eq!(f.http_status(), 503);
            assert_eq!(f.retry_after_secs(0), Some(60));
        }
    }

    /// Everything the previous boolean called an outage still is one, with the
    /// same status and the same hint. This class is the one A1 does not touch.
    #[test]
    fn node_level_failures_keep_their_old_answer() {
        for fixture in [
            r#"ContractCall("ErrorResp(ErrorPayload { code: -32000, message: \"header not found\" })")"#,
            r#"error code -32000: historical state fa81e909 is not available"#,
            r#"ErrorResp(ErrorPayload { code: -32801, message: "no historical RPC is available for this historical (pre-L2) execution request" })"#,
            r#"ContractCall("ErrorResp(ErrorPayload { code: -32603, message: \"json: unsupported value\" })")"#,
            r#"Transport(Custom("Max retries exceeded server returned an error response"))"#,
        ] {
            let f = classify(fixture);
            assert_eq!(f.reason, Reason::Transport, "{fixture}");
            assert_eq!(f.http_status(), 502);
            assert_eq!(f.retry_after_secs(0), Some(30));
        }
    }

    /// A revert is an answer from the chain, so it stays the caller's to fix —
    /// including when it is wrapped in a transport error that carries a node
    /// code, and including when its own text mentions funds.
    #[test]
    fn a_revert_stays_the_callers_problem() {
        for fixture in [
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted: FiatTokenV2: invalid signature" })"#,
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted: ERC20: transfer amount exceeds balance" })"#,
            r#"Transport(Custom("... code: -32000 ... execution reverted: FiatTokenV2: invalid signature"))"#,
            r#"ErrorResp(ErrorPayload { code: 3, message: "execution reverted: Vault: insufficient funds in pool" })"#,
            r#"ContractCall("transaction 0x1111111111111111111111111111111111111111111111111111111111111111 reverted on polygon")"#,
        ] {
            let f = classify(fixture);
            assert_eq!(f.reason, Reason::PayloadRejected, "{fixture}");
            assert_eq!(f.http_status(), 400);
            assert_eq!(f.retry_after_secs(0), None);
        }
    }

    /// `txpool is full` never entered the mempool, so a retry is correct — and
    /// it is named as a mempool refusal rather than as an outage.
    #[test]
    fn a_full_mempool_is_named_and_retryable() {
        let f = classify(r#"ErrorResp(ErrorPayload { code: -32003, message: "txpool is full" })"#);
        assert_eq!(f.reason, Reason::NonceOrMempool);
        assert_eq!(f.http_status(), 502);
        assert_eq!(f.retry_after_secs(0), Some(30));
    }

    /// `-32003` is overloaded: `eth_call`'s out-of-gas rejection carries it too
    /// and is a real answer about the request, not an outage.
    #[test]
    fn out_of_gas_is_not_a_mempool_refusal() {
        let f = classify(
            "server returned an error response: error code -32003: out of gas: \
             gas exhausted during memory expansion: 600000000",
        );
        assert_ne!(f.reason, Reason::NonceOrMempool);
        assert_eq!(f.http_status(), 400);
    }

    /// Unrecognised text keeps the conservative 400.
    #[test]
    fn unknown_text_is_not_promoted() {
        for fixture in ["SchemeMismatch", "something entirely new"] {
            let f = classify(fixture);
            assert_eq!(f.reason, Reason::Unclassified, "{fixture}");
            assert_eq!(f.http_status(), 400);
            assert_eq!(f.retry_after_secs(0), None);
        }
    }

    /// The figures the operator needs, off the real message: usable margin is
    /// 0.0623 POL against a transaction costing 0.0785, while the balance alone
    /// reads as a healthy 82.86.
    #[test]
    fn the_shortfall_figures_come_out_of_the_real_message() {
        let s = GasShortfall::parse(REAL_UNFUNDED);
        assert_eq!(s.balance, Some(82_861_633_384_675_957_709));
        assert_eq!(s.queued_cost, Some(82_799_377_752_610_042_973));
        assert_eq!(s.tx_cost, Some(78_463_640_160_630_732));
        assert_eq!(s.overshot, Some(16_208_008_094_715_996));
        assert_eq!(s.usable(), Some(62_255_632_065_914_736));
        assert!(
            s.usable().unwrap() < s.tx_cost.unwrap(),
            "this is the whole point: a large balance with no usable margin"
        );
    }

    /// The `have`/`want` phrasing yields the same two numbers.
    #[test]
    fn the_plainer_phrasing_parses_too() {
        let s = GasShortfall::parse(REAL_UNFUNDED_HAVE_WANT);
        assert_eq!(s.balance, Some(62_255_632_065_914_740));
        assert_eq!(s.tx_cost, Some(78_463_640_160_630_732));
        assert_eq!(s.queued_cost, None);
        assert_eq!(s.usable(), Some(62_255_632_065_914_740));
    }

    /// A message with no figures must not invent any.
    #[test]
    fn nothing_is_invented_when_the_message_carries_no_figures() {
        let s = GasShortfall::parse("insufficient funds for gas * price + value");
        assert!(s.is_empty());
        assert_eq!(s.usable(), None);
    }

    /// Wei overflows `u64`; a parser that used one would report a wrong number
    /// rather than none.
    #[test]
    fn wei_amounts_survive_being_larger_than_u64() {
        let big = "balance 340282366920938463463374607431768211455,";
        assert_eq!(GasShortfall::parse(big).balance, Some(u128::MAX));
        let too_big = "balance 3402823669209384634633746074317682114550,";
        assert_eq!(GasShortfall::parse(too_big).balance, None);
    }
}
