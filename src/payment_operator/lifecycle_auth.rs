//! Signed lifecycle orders for `release` and `refundInEscrow`.
//!
//! Both actions move money that is already escrowed, so neither carries an
//! ERC-3009 signature: there is no transfer left to authorize. That leaves
//! the other half of the question open -- who is entitled to ask for the
//! move -- and this module answers it, so the entitlement rides on a key
//! instead of on whatever the caller asserts about itself. Escrow exists
//! because both sides can lose real money to a lifecycle action they did not
//! consent to; the table below is which consent counts for which action.
//!
//! Entitlement is an EIP-712 order signed by the party the action belongs to:
//!
//! | action           | accepted signers                                              |
//! |------------------|---------------------------------------------------------------|
//! | `release`        | `paymentInfo.payer`; the operator owner (`FEE_RECIPIENT()`)   |
//! | `refundInEscrow` | `paymentInfo.receiver`; the operator owner; the payer, but only once `authorizationExpiry` has passed (the chain already lets them `reclaim()`) |
//!
//! The receiver may never release (self-payment is what escrow exists to stop)
//! and the payer may never refund before expiry (that is the chargeback).
//!
//! The order is signed over the SAME `paymentInfo` that is submitted, so
//! neither side computes `getHash` and there is nothing to drift. `PaymentInfo`
//! below is the AuthCaptureEscrow type string verbatim
//! (`PAYMENT_INFO_TYPEHASH` in Coinbase Commerce Payments), so a client that
//! already types it for ERC-3009 reuses it.
//!
//! Rollout is by [`ENV_MODE`]: `off` (default: the order is not checked), `log`
//! (verify when present, log the verdict, never reject) and `enforce`. A
//! garbage value is `off` plus a warning, never a refusal to boot. The
//! effective mode is published on `GET /settle`.
//!
//! "No verdict" is a refusal here. When the operator owner cannot be read
//! from the chain, an owner claim is rejected as `owner_unverifiable`
//! (retryable), the opposite of the ERC-8004 proof gate: a rating that goes
//! unchecked costs reputation, an escrow order that goes unchecked costs money.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::chain::evm::{EvmChain, EvmProvider};
use crate::network::Network;

use super::abi::OperatorContract;
use super::errors::OperatorError;
use super::types::{ContractPaymentInfo, EscrowLifecyclePayload};

/// `off` | `log` | `enforce`. Anything else is `off` with a warning.
pub const ENV_MODE: &str = "ESCROW_LIFECYCLE_AUTH";

/// Upper bound on how far ahead an order's `deadline` may sit.
pub const ENV_MAX_DEADLINE_SECS: &str = "ESCROW_LIFECYCLE_MAX_DEADLINE_SECS";

/// Fifteen minutes, the same window the ERC-8004 relay gives a signed
/// authorisation: long enough to sign and send, short enough that a leaked
/// order is not a standing permission.
pub const DEFAULT_MAX_DEADLINE_SECS: u64 = 900;

/// EIP-712 domain. `chainId` is the payment network's; there is no
/// `verifyingContract` because `paymentInfo.operator` is inside the signed
/// struct already.
pub const DOMAIN_NAME: &str = "x402 escrow lifecycle";
pub const DOMAIN_VERSION: &str = "1";

sol! {
    /// AuthCaptureEscrow.PaymentInfo, field order and types verbatim. It is
    /// part of the type hash: reordering invalidates every issued order.
    #[derive(Debug)]
    struct PaymentInfo {
        address operator;
        address payer;
        address receiver;
        address token;
        uint120 maxAmount;
        uint48 preApprovalExpiry;
        uint48 authorizationExpiry;
        uint48 refundExpiry;
        uint16 minFeeBps;
        uint16 maxFeeBps;
        address feeReceiver;
        uint256 salt;
    }

    /// The order. `action` is the wire value (`release` / `refundInEscrow`),
    /// `amount` the exact amount submitted, `nonce` a caller-chosen 32-byte
    /// value that is consumed on success.
    #[derive(Debug)]
    struct LifecycleOrder {
        string action;
        uint256 amount;
        uint256 deadline;
        bytes32 nonce;
        PaymentInfo paymentInfo;
    }
}

// ============================================================================
// Wire types
// ============================================================================

/// `payload.lifecycleAuth` on a `release` / `refundInEscrow` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleAuth {
    /// Who claims to have signed. Checked against the recovered address.
    pub signer: Address,
    /// Unix seconds after which the order is dead.
    pub deadline: u64,
    /// Caller-chosen replay guard.
    pub nonce: B256,
    /// 65-byte EIP-712 signature over [`LifecycleOrder`].
    pub signature: Bytes,
}

/// The two actions this module guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Release,
    RefundInEscrow,
}

impl LifecycleAction {
    /// The wire value, which is also what goes into the signed order.
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleAction::Release => "release",
            LifecycleAction::RefundInEscrow => "refundInEscrow",
        }
    }
}

// ============================================================================
// Mode
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Log,
    Enforce,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Log => "log",
            Mode::Enforce => "enforce",
        }
    }
}

/// Parse the mode. The second element is a warning to log when the value was
/// not understood; the mode is then `Off`, never an error.
pub fn parse_mode(raw: Option<&str>) -> (Mode, Option<String>) {
    let Some(raw) = raw else {
        return (Mode::Off, None);
    };
    let value = raw.trim();
    if value.eq_ignore_ascii_case("off") || value.is_empty() {
        (Mode::Off, None)
    } else if value.eq_ignore_ascii_case("log") {
        (Mode::Log, None)
    } else if value.eq_ignore_ascii_case("enforce") {
        (Mode::Enforce, None)
    } else {
        (
            Mode::Off,
            Some(format!(
                "{ENV_MODE}={value:?} is not one of off|log|enforce; running with off"
            )),
        )
    }
}

/// The effective mode, from the environment.
pub fn mode() -> Mode {
    let (mode, warning) = parse_mode(std::env::var(ENV_MODE).ok().as_deref());
    if let Some(w) = warning {
        warn!("{w}");
    }
    mode
}

/// Deadline ceiling, from the environment or [`DEFAULT_MAX_DEADLINE_SECS`].
pub fn max_deadline_secs() -> u64 {
    std::env::var(ENV_MAX_DEADLINE_SECS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_DEADLINE_SECS)
}

// ============================================================================
// Verdicts
// ============================================================================

/// Which rule admitted the signer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Payer,
    Receiver,
    OperatorOwner,
    /// The payer, refunding after `authorizationExpiry`.
    PayerAfterExpiry,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Payer => "payer",
            Role::Receiver => "receiver",
            Role::OperatorOwner => "operator_owner",
            Role::PayerAfterExpiry => "payer_after_expiry",
        }
    }
}

/// The outcome of checking one order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok {
        signer: Address,
        role: Role,
    },
    /// No `lifecycleAuth` in the payload.
    Missing,
    /// Malformed, or recovers to an address other than the claimed `signer`.
    BadSignature,
    /// `deadline` is in the past.
    Expired,
    /// `deadline` is further ahead than the ceiling allows.
    DeadlineTooFar,
    /// The nonce was already consumed.
    Replayed,
    /// The signature is real but the signer holds no role for this action.
    UnauthorizedRole {
        signer: Address,
    },
    /// The signer could only be the operator owner and the owner could not
    /// be read from the chain. Retryable.
    OwnerUnverifiable {
        signer: Address,
        error: String,
    },
}

impl Verdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, Verdict::Ok { .. })
    }

    /// Bounded label for logs and error bodies.
    pub fn category(&self) -> &'static str {
        match self {
            Verdict::Ok { .. } => "ok",
            Verdict::Missing => "missing",
            Verdict::BadSignature => "bad_signature",
            Verdict::Expired => "expired",
            Verdict::DeadlineTooFar => "deadline_too_far",
            Verdict::Replayed => "replayed",
            Verdict::UnauthorizedRole { .. } => "unauthorized_role",
            Verdict::OwnerUnverifiable { .. } => "owner_unverifiable",
        }
    }
}

/// What the request looks like to the policy. Pure data so the policy can be
/// tested without a chain.
#[derive(Debug, Clone)]
pub struct OrderContext<'a> {
    pub action: LifecycleAction,
    pub payment_info: &'a ContractPaymentInfo,
    pub amount: u128,
    pub chain_id: u64,
    /// Unix seconds.
    pub now: u64,
    pub max_deadline_secs: u64,
}

/// The first, chain-free half of the evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Settled without reading the chain.
    Verdict(Verdict),
    /// The signature is good and fresh but the signer is neither payer nor
    /// receiver (or holds no role as such). Only the operator owner rule is
    /// left, and that one needs a read.
    NeedsOwner { signer: Address },
}

// ============================================================================
// Hashing and recovery
// ============================================================================

fn to_sol_payment_info(p: &ContractPaymentInfo) -> PaymentInfo {
    use alloy::primitives::Uint;
    PaymentInfo {
        operator: p.operator,
        payer: p.payer,
        receiver: p.receiver,
        token: p.token,
        maxAmount: Uint::from(p.max_amount),
        preApprovalExpiry: Uint::from(p.pre_approval_expiry),
        authorizationExpiry: Uint::from(p.authorization_expiry),
        refundExpiry: Uint::from(p.refund_expiry),
        minFeeBps: p.min_fee_bps,
        maxFeeBps: p.max_fee_bps,
        feeReceiver: p.fee_receiver,
        salt: p.salt,
    }
}

/// The EIP-712 digest a signer must sign for this order.
pub fn signing_hash(
    action: LifecycleAction,
    amount: u128,
    deadline: u64,
    nonce: B256,
    payment_info: &ContractPaymentInfo,
    chain_id: u64,
) -> B256 {
    let order = LifecycleOrder {
        action: action.as_str().to_string(),
        amount: U256::from(amount),
        deadline: U256::from(deadline),
        nonce,
        paymentInfo: to_sol_payment_info(payment_info),
    };
    let domain = eip712_domain! {
        name: DOMAIN_NAME,
        version: DOMAIN_VERSION,
        chain_id: chain_id,
    };
    order.eip712_signing_hash(&domain)
}

/// Recover the signer of `auth` for `digest`. `None` when malformed.
pub fn recover(auth: &LifecycleAuth, digest: B256) -> Option<Address> {
    let sig = alloy::primitives::Signature::try_from(auth.signature.as_ref()).ok()?;
    sig.recover_address_from_prehash(&digest).ok()
}

// ============================================================================
// Policy
// ============================================================================

/// Which role, if any, admits `signer` to `action` without reading the chain.
fn local_role(ctx: &OrderContext<'_>, signer: Address) -> Option<Role> {
    let p = ctx.payment_info;
    match ctx.action {
        LifecycleAction::Release => (signer == p.payer).then_some(Role::Payer),
        LifecycleAction::RefundInEscrow => {
            if signer == p.receiver {
                Some(Role::Receiver)
            } else if signer == p.payer && ctx.now >= p.authorization_expiry {
                Some(Role::PayerAfterExpiry)
            } else {
                None
            }
        }
    }
}

/// Evaluate everything that does not need the chain.
///
/// Consumes the nonce through `replay` only when the order is otherwise
/// valid, so a rejected order does not burn a nonce the caller may resend
/// once it is fixed.
pub fn pre_evaluate(
    auth: Option<&LifecycleAuth>,
    ctx: &OrderContext<'_>,
    replay: &ReplayGuard,
) -> Step {
    let Some(auth) = auth else {
        return Step::Verdict(Verdict::Missing);
    };
    if auth.deadline < ctx.now {
        return Step::Verdict(Verdict::Expired);
    }
    if auth.deadline - ctx.now > ctx.max_deadline_secs {
        return Step::Verdict(Verdict::DeadlineTooFar);
    }
    let digest = signing_hash(
        ctx.action,
        ctx.amount,
        auth.deadline,
        auth.nonce,
        ctx.payment_info,
        ctx.chain_id,
    );
    match recover(auth, digest) {
        Some(recovered) if recovered == auth.signer => {}
        _ => return Step::Verdict(Verdict::BadSignature),
    }
    if !replay.check_and_insert(auth.nonce, auth.deadline, ctx.now) {
        return Step::Verdict(Verdict::Replayed);
    }
    match local_role(ctx, auth.signer) {
        Some(role) => Step::Verdict(Verdict::Ok {
            signer: auth.signer,
            role,
        }),
        None => Step::NeedsOwner {
            signer: auth.signer,
        },
    }
}

/// Finish a [`Step::NeedsOwner`] with what the chain said the owner is.
pub fn finish_with_owner(signer: Address, owner: Result<Address, String>) -> Verdict {
    match owner {
        Ok(owner) if owner == signer => Verdict::Ok {
            signer,
            role: Role::OperatorOwner,
        },
        Ok(_) => Verdict::UnauthorizedRole { signer },
        Err(error) => Verdict::OwnerUnverifiable { signer, error },
    }
}

/// Turn a verdict into a go/no-go for the mode.
pub fn decide(mode: Mode, verdict: &Verdict) -> Result<(), OperatorError> {
    match mode {
        Mode::Off | Mode::Log => Ok(()),
        Mode::Enforce => {
            if verdict.is_ok() {
                Ok(())
            } else {
                Err(OperatorError::LifecycleAuthRejected {
                    category: verdict.category(),
                    detail: describe(verdict),
                })
            }
        }
    }
}

fn describe(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Ok { signer, role } => format!("signed by {signer} as {}", role.as_str()),
        Verdict::Missing => {
            "payload.lifecycleAuth is required for release and refundInEscrow".to_string()
        }
        Verdict::BadSignature => {
            "signature is malformed or does not recover to lifecycleAuth.signer".to_string()
        }
        Verdict::Expired => "lifecycleAuth.deadline is in the past".to_string(),
        Verdict::DeadlineTooFar => "lifecycleAuth.deadline is too far ahead".to_string(),
        Verdict::Replayed => "lifecycleAuth.nonce was already used".to_string(),
        Verdict::UnauthorizedRole { signer } => {
            format!("{signer} is neither a party to this escrow nor the operator owner")
        }
        Verdict::OwnerUnverifiable { signer, error } => {
            format!("could not read the operator owner to check {signer}; retry later ({error})")
        }
    }
}

// ============================================================================
// Replay guard
// ============================================================================

/// Nonces consumed by accepted orders, kept until their deadline passes.
///
/// Per process. EVM writes are funnelled to the lease holder by
/// `settle_writer_gate`, so in practice one process sees every order; the
/// window that survives a deploy is bounded by the deadline ceiling.
#[derive(Debug, Default)]
pub struct ReplayGuard {
    seen: Mutex<HashMap<B256, u64>>,
}

impl ReplayGuard {
    /// `true` when `nonce` was fresh and is now consumed.
    pub fn check_and_insert(&self, nonce: B256, expires_at: u64, now: u64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() > 4096 {
            seen.retain(|_, exp| *exp >= now);
        }
        match seen.get(&nonce) {
            Some(exp) if *exp >= now => false,
            _ => {
                seen.insert(nonce, expires_at);
                true
            }
        }
    }
}

fn replay_guard() -> &'static ReplayGuard {
    static GUARD: OnceLock<ReplayGuard> = OnceLock::new();
    GUARD.get_or_init(ReplayGuard::default)
}

// ============================================================================
// Operator owner
// ============================================================================

fn owner_cache() -> &'static Mutex<HashMap<(u64, Address), Address>> {
    static CACHE: OnceLock<Mutex<HashMap<(u64, Address), Address>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `FEE_RECIPIENT()` of the operator: immutable on every deployed operator,
/// so it is read once per (chain, operator) and cached for the process.
async fn read_operator_owner(
    provider: &EvmProvider,
    chain_id: u64,
    operator: Address,
) -> Result<Address, String> {
    if let Some(a) = owner_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(chain_id, operator))
    {
        return Ok(*a);
    }
    let call = OperatorContract::FEE_RECIPIENTCall {};
    let raw = super::operator::eth_call(provider, operator, &call)
        .await
        .map_err(|e| e.to_string())?;
    let owner: Address = OperatorContract::FEE_RECIPIENTCall::abi_decode_returns(&raw)
        .map_err(|e| format!("decode FEE_RECIPIENT: {e}"))?;
    owner_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((chain_id, operator), owner);
    Ok(owner)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// The gate
// ============================================================================

/// Check the order on a `release` / `refundInEscrow` request under the
/// configured mode. Returns before touching anything when the mode is `off`.
pub async fn gate(
    action: LifecycleAction,
    lifecycle: &EscrowLifecyclePayload,
    network: Network,
    provider: &EvmProvider,
) -> Result<(), OperatorError> {
    let mode = mode();
    if mode == Mode::Off {
        return Ok(());
    }
    let chain_id = EvmChain::try_from(network)
        .map_err(|_| OperatorError::NonEvmNetwork)?
        .chain_id;
    let payment_info = ContractPaymentInfo::from_lifecycle_payload(lifecycle);
    let ctx = OrderContext {
        action,
        payment_info: &payment_info,
        amount: lifecycle.amount,
        chain_id,
        now: unix_now(),
        max_deadline_secs: max_deadline_secs(),
    };
    let verdict = match pre_evaluate(lifecycle.lifecycle_auth.as_ref(), &ctx, replay_guard()) {
        Step::Verdict(v) => v,
        Step::NeedsOwner { signer } => {
            let owner = read_operator_owner(provider, chain_id, payment_info.operator).await;
            finish_with_owner(signer, owner)
        }
    };
    let signer = match &verdict {
        Verdict::Ok { signer, .. }
        | Verdict::UnauthorizedRole { signer }
        | Verdict::OwnerUnverifiable { signer, .. } => Some(*signer),
        _ => lifecycle.lifecycle_auth.as_ref().map(|a| a.signer),
    };
    if verdict.is_ok() {
        info!(
            action = action.as_str(),
            network = %network,
            mode = mode.as_str(),
            verdict = verdict.category(),
            signer = ?signer,
            payer = ?payment_info.payer,
            receiver = ?payment_info.receiver,
            "escrow lifecycle order accepted"
        );
    } else {
        warn!(
            action = action.as_str(),
            network = %network,
            mode = mode.as_str(),
            verdict = verdict.category(),
            signer = ?signer,
            payer = ?payment_info.payer,
            receiver = ?payment_info.receiver,
            detail = %describe(&verdict),
            "escrow lifecycle order NOT authorized"
        );
    }
    decide(mode, &verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;

    const NOW: u64 = 1_757_000_000;
    const CHAIN: u64 = 8453;

    fn payment_info(
        payer: Address,
        receiver: Address,
        authorization_expiry: u64,
    ) -> ContractPaymentInfo {
        ContractPaymentInfo {
            operator: address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb"),
            payer,
            receiver,
            token: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            max_amount: 1_000_000,
            pre_approval_expiry: NOW + 3600,
            authorization_expiry,
            refund_expiry: NOW + 30 * 86400,
            min_fee_bps: 0,
            max_fee_bps: 1300,
            fee_receiver: address!("aE07cEB6b395BC685a776a0b4c489E8d9cE9A6ad"),
            salt: U256::from(12345u64),
        }
    }

    fn ctx<'a>(action: LifecycleAction, payment_info: &'a ContractPaymentInfo) -> OrderContext<'a> {
        OrderContext {
            action,
            payment_info,
            amount: 1_000_000,
            chain_id: CHAIN,
            now: NOW,
            max_deadline_secs: DEFAULT_MAX_DEADLINE_SECS,
        }
    }

    fn sign(
        signer: &PrivateKeySigner,
        ctx: &OrderContext<'_>,
        deadline: u64,
        nonce: B256,
    ) -> LifecycleAuth {
        let digest = signing_hash(
            ctx.action,
            ctx.amount,
            deadline,
            nonce,
            ctx.payment_info,
            ctx.chain_id,
        );
        let sig = signer.sign_hash_sync(&digest).expect("sign");
        LifecycleAuth {
            signer: signer.address(),
            deadline,
            nonce,
            signature: Bytes::copy_from_slice(&sig.as_bytes()),
        }
    }

    fn nonce(n: u8) -> B256 {
        B256::repeat_byte(n)
    }

    // ---- mode ------------------------------------------------------------

    #[test]
    fn mode_defaults_to_off_and_garbage_is_off_with_a_warning() {
        assert_eq!(parse_mode(None), (Mode::Off, None));
        assert_eq!(parse_mode(Some("off")), (Mode::Off, None));
        assert_eq!(parse_mode(Some("LOG")).0, Mode::Log);
        assert_eq!(parse_mode(Some(" enforce ")).0, Mode::Enforce);
        let (mode, warning) = parse_mode(Some("banana"));
        assert_eq!(mode, Mode::Off);
        assert!(warning.expect("warning").contains("banana"));
    }

    /// The default is the *absence* of configuration, so read it from the real
    /// environment rather than from [`parse_mode`]: with `ESCROW_LIFECYCLE_AUTH`
    /// unset the gate is off and a request carrying no order is admitted exactly
    /// as it is today. Discriminating -- setting the variable is the only thing
    /// that moves the verdict, and the second half of this test is what keeps
    /// the first half from passing vacuously.
    #[test]
    fn an_unset_environment_leaves_the_gate_off() {
        // Process-global; safe under CI's `--test-threads=1` and restored below.
        let previous = std::env::var(ENV_MODE).ok();
        std::env::remove_var(ENV_MODE);
        assert_eq!(mode(), Mode::Off);
        assert!(decide(mode(), &Verdict::Missing).is_ok());

        std::env::set_var(ENV_MODE, "enforce");
        assert_eq!(mode(), Mode::Enforce);
        assert!(decide(mode(), &Verdict::Missing).is_err());

        match previous {
            Some(v) => std::env::set_var(ENV_MODE, v),
            None => std::env::remove_var(ENV_MODE),
        }
    }

    #[test]
    fn off_and_log_never_reject_and_enforce_rejects_anything_but_ok() {
        let bad = Verdict::Missing;
        let ok = Verdict::Ok {
            signer: Address::ZERO,
            role: Role::Payer,
        };
        assert!(decide(Mode::Off, &bad).is_ok());
        assert!(decide(Mode::Log, &bad).is_ok());
        assert!(decide(Mode::Enforce, &ok).is_ok());
        match decide(Mode::Enforce, &bad) {
            Err(OperatorError::LifecycleAuthRejected { category, .. }) => {
                assert_eq!(category, "missing")
            }
            other => panic!("expected LifecycleAuthRejected, got {other:?}"),
        }
    }

    // ---- wire ------------------------------------------------------------

    #[test]
    fn lifecycle_payload_parses_with_and_without_auth() {
        let without = r#"{
            "paymentInfo": {
                "operator": "0x271f9fa7f8907aCf178CCFB470076D9129D8F0Eb",
                "receiver": "0x2222222222222222222222222222222222222222",
                "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "maxAmount": "1000000",
                "preApprovalExpiry": 1, "authorizationExpiry": 2, "refundExpiry": 3,
                "minFeeBps": 0, "maxFeeBps": 1300,
                "feeReceiver": "0xaE07cEB6b395BC685a776a0b4c489E8d9cE9A6ad",
                "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
            },
            "payer": "0x1111111111111111111111111111111111111111",
            "amount": "1000000"
        }"#;
        let parsed: EscrowLifecyclePayload = serde_json::from_str(without).expect("parses");
        assert!(parsed.lifecycle_auth.is_none());

        let with = format!(
            r#"{}, "lifecycleAuth": {{
                "signer": "0x1111111111111111111111111111111111111111",
                "deadline": 1757000600,
                "nonce": "{}",
                "signature": "0x00"
            }}}}"#,
            without.trim_end().trim_end_matches('}'),
            nonce(1)
        );
        let parsed: EscrowLifecyclePayload = serde_json::from_str(&with).expect("parses");
        let auth = parsed.lifecycle_auth.expect("auth present");
        assert_eq!(auth.deadline, 1_757_000_600);
        assert_eq!(auth.nonce, nonce(1));
    }

    // ---- hashing ---------------------------------------------------------

    #[test]
    fn the_digest_binds_action_amount_nonce_deadline_and_chain() {
        let pi = payment_info(Address::ZERO, Address::ZERO, NOW);
        let base = signing_hash(LifecycleAction::Release, 1, 10, nonce(1), &pi, CHAIN);
        assert_eq!(
            base,
            signing_hash(LifecycleAction::Release, 1, 10, nonce(1), &pi, CHAIN)
        );
        assert_ne!(
            base,
            signing_hash(LifecycleAction::RefundInEscrow, 1, 10, nonce(1), &pi, CHAIN)
        );
        assert_ne!(
            base,
            signing_hash(LifecycleAction::Release, 2, 10, nonce(1), &pi, CHAIN)
        );
        assert_ne!(
            base,
            signing_hash(LifecycleAction::Release, 1, 11, nonce(1), &pi, CHAIN)
        );
        assert_ne!(
            base,
            signing_hash(LifecycleAction::Release, 1, 10, nonce(2), &pi, CHAIN)
        );
        assert_ne!(
            base,
            signing_hash(LifecycleAction::Release, 1, 10, nonce(1), &pi, 1)
        );
        let mut other = pi.clone();
        other.salt = U256::from(1u64);
        assert_ne!(
            base,
            signing_hash(LifecycleAction::Release, 1, 10, nonce(1), &other, CHAIN)
        );
    }

    /// Digest and signature computed OUTSIDE this code, with web3.py's
    /// `eth_account.encode_typed_data` over the same types, so the vector is
    /// not this implementation compared to itself. The key is the canonical
    /// test key of thirty-two 0x01 bytes (address 0x1a64...14F1), which is
    /// what an EM or SDK integrator will reproduce first.
    #[test]
    fn the_digest_matches_an_independent_eth_account_vector() {
        use alloy::primitives::b256;
        let pi = payment_info(
            address!("1111111111111111111111111111111111111111"),
            address!("2222222222222222222222222222222222222222"),
            NOW + 3600,
        );
        let digest = signing_hash(
            LifecycleAction::Release,
            1_000_000,
            NOW + 60,
            nonce(1),
            &pi,
            CHAIN,
        );
        assert_eq!(
            digest,
            b256!("69a40a6c3f34c059aa6b400edca18a853272e704ea978e47e0fc38dacdd5cad9")
        );
        let raw = hex::decode(
            "09df3dc12e80fcfcc426ab39668a3be0c28fe6323a2acaeec57e55e9b95964f2\
             5ec2b8909b29b165d1f1a5dd3d0805880b0db67d7a541f423cdc772ea06129051c",
        )
        .expect("hex");
        let auth = LifecycleAuth {
            signer: address!("1a642f0E3c3aF545E7AcBD38b07251B3990914F1"),
            deadline: NOW + 60,
            nonce: nonce(1),
            signature: Bytes::from(raw),
        };
        assert_eq!(recover(&auth, digest), Some(auth.signer));
        let c = ctx(LifecycleAction::Release, &pi);
        // The payer is 0x1111..., so this signer falls through to the owner rule.
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::NeedsOwner {
                signer: auth.signer
            }
        );
    }

    // ---- policy: release -------------------------------------------------

    #[test]
    fn the_payer_may_release() {
        let payer = PrivateKeySigner::random();
        let pi = payment_info(
            payer.address(),
            address!("2222222222222222222222222222222222222222"),
            NOW + 3600,
        );
        let c = ctx(LifecycleAction::Release, &pi);
        let auth = sign(&payer, &c, NOW + 60, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::Ok {
                signer: payer.address(),
                role: Role::Payer
            })
        );
    }

    #[test]
    fn the_receiver_may_not_release_itself_and_falls_through_to_the_owner_rule() {
        let receiver = PrivateKeySigner::random();
        let pi = payment_info(
            address!("1111111111111111111111111111111111111111"),
            receiver.address(),
            NOW + 3600,
        );
        let c = ctx(LifecycleAction::Release, &pi);
        let auth = sign(&receiver, &c, NOW + 60, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::NeedsOwner {
                signer: receiver.address()
            }
        );
        assert_eq!(
            finish_with_owner(receiver.address(), Ok(pi.fee_receiver)),
            Verdict::UnauthorizedRole {
                signer: receiver.address()
            }
        );
    }

    // ---- policy: refundInEscrow -----------------------------------------

    #[test]
    fn the_payer_may_not_refund_before_expiry_that_is_the_chargeback() {
        let payer = PrivateKeySigner::random();
        let pi = payment_info(
            payer.address(),
            address!("2222222222222222222222222222222222222222"),
            NOW + 3600,
        );
        let c = ctx(LifecycleAction::RefundInEscrow, &pi);
        let auth = sign(&payer, &c, NOW + 60, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::NeedsOwner {
                signer: payer.address()
            }
        );
        assert_eq!(
            finish_with_owner(payer.address(), Ok(pi.fee_receiver)),
            Verdict::UnauthorizedRole {
                signer: payer.address()
            }
        );
    }

    #[test]
    fn the_payer_may_refund_once_the_authorization_expired() {
        let payer = PrivateKeySigner::random();
        let pi = payment_info(
            payer.address(),
            address!("2222222222222222222222222222222222222222"),
            NOW - 1,
        );
        let c = ctx(LifecycleAction::RefundInEscrow, &pi);
        let auth = sign(&payer, &c, NOW + 60, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::Ok {
                signer: payer.address(),
                role: Role::PayerAfterExpiry
            })
        );
    }

    #[test]
    fn the_receiver_may_refund_voluntarily() {
        let receiver = PrivateKeySigner::random();
        let pi = payment_info(
            address!("1111111111111111111111111111111111111111"),
            receiver.address(),
            NOW + 3600,
        );
        let c = ctx(LifecycleAction::RefundInEscrow, &pi);
        let auth = sign(&receiver, &c, NOW + 60, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::Ok {
                signer: receiver.address(),
                role: Role::Receiver
            })
        );
    }

    // ---- policy: operator owner -----------------------------------------

    #[test]
    fn the_operator_owner_may_release_and_refund_and_no_verdict_is_a_refusal() {
        let owner = PrivateKeySigner::random();
        let pi = payment_info(
            address!("1111111111111111111111111111111111111111"),
            address!("2222222222222222222222222222222222222222"),
            NOW + 3600,
        );
        for action in [LifecycleAction::Release, LifecycleAction::RefundInEscrow] {
            let c = ctx(action, &pi);
            let auth = sign(&owner, &c, NOW + 60, nonce(7));
            let step = pre_evaluate(Some(&auth), &c, &ReplayGuard::default());
            assert_eq!(
                step,
                Step::NeedsOwner {
                    signer: owner.address()
                }
            );
            assert_eq!(
                finish_with_owner(owner.address(), Ok(owner.address())),
                Verdict::Ok {
                    signer: owner.address(),
                    role: Role::OperatorOwner
                }
            );
            let unverifiable = finish_with_owner(owner.address(), Err("rpc down".into()));
            assert_eq!(unverifiable.category(), "owner_unverifiable");
            assert!(decide(Mode::Enforce, &unverifiable).is_err());
        }
    }

    // ---- freshness, identity, replay ------------------------------------

    #[test]
    fn a_stale_or_far_deadline_is_refused_before_any_signature_work() {
        let payer = PrivateKeySigner::random();
        let pi = payment_info(payer.address(), Address::ZERO, NOW + 3600);
        let c = ctx(LifecycleAction::Release, &pi);
        let stale = sign(&payer, &c, NOW - 1, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&stale), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::Expired)
        );
        let far = sign(&payer, &c, NOW + DEFAULT_MAX_DEADLINE_SECS + 1, nonce(2));
        assert_eq!(
            pre_evaluate(Some(&far), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::DeadlineTooFar)
        );
    }

    #[test]
    fn a_signature_by_someone_else_under_the_payers_name_is_bad_not_unauthorized() {
        let payer = PrivateKeySigner::random();
        let stranger = PrivateKeySigner::random();
        let pi = payment_info(payer.address(), Address::ZERO, NOW + 3600);
        let c = ctx(LifecycleAction::Release, &pi);
        let mut forged = sign(&stranger, &c, NOW + 60, nonce(1));
        forged.signer = payer.address();
        assert_eq!(
            pre_evaluate(Some(&forged), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::BadSignature)
        );
        let mut garbage = sign(&payer, &c, NOW + 60, nonce(2));
        garbage.signature = Bytes::from_static(&[0u8; 3]);
        assert_eq!(
            pre_evaluate(Some(&garbage), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::BadSignature)
        );
    }

    #[test]
    fn an_order_signed_for_another_amount_does_not_authorize_this_one() {
        let payer = PrivateKeySigner::random();
        let pi = payment_info(payer.address(), Address::ZERO, NOW + 3600);
        let mut c = ctx(LifecycleAction::Release, &pi);
        let auth = sign(&payer, &c, NOW + 60, nonce(1));
        c.amount = 999_999;
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::BadSignature)
        );
    }

    #[test]
    fn a_nonce_is_consumed_by_an_accepted_order_and_not_by_a_rejected_one() {
        let payer = PrivateKeySigner::random();
        let pi = payment_info(payer.address(), Address::ZERO, NOW + 3600);
        let c = ctx(LifecycleAction::Release, &pi);
        let guard = ReplayGuard::default();
        let auth = sign(&payer, &c, NOW + 60, nonce(1));
        assert!(pre_evaluate(Some(&auth), &c, &guard) != Step::Verdict(Verdict::Replayed));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &guard),
            Step::Verdict(Verdict::Replayed)
        );
        // A bad signature never touches the guard.
        let mut forged = sign(&payer, &c, NOW + 60, nonce(2));
        forged.signature = Bytes::from_static(&[0u8; 65]);
        assert_eq!(
            pre_evaluate(Some(&forged), &c, &guard),
            Step::Verdict(Verdict::BadSignature)
        );
        let fresh = sign(&payer, &c, NOW + 60, nonce(2));
        assert!(pre_evaluate(Some(&fresh), &c, &guard).is_ok_verdict());
    }

    #[test]
    fn missing_auth_is_its_own_verdict() {
        let pi = payment_info(Address::ZERO, Address::ZERO, NOW);
        let c = ctx(LifecycleAction::Release, &pi);
        assert_eq!(
            pre_evaluate(None, &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::Missing)
        );
    }

    #[test]
    fn the_replay_guard_forgets_expired_nonces() {
        let guard = ReplayGuard::default();
        assert!(guard.check_and_insert(nonce(1), NOW + 10, NOW));
        assert!(!guard.check_and_insert(nonce(1), NOW + 10, NOW + 5));
        assert!(guard.check_and_insert(nonce(1), NOW + 100, NOW + 11));
    }

    impl Step {
        fn is_ok_verdict(&self) -> bool {
            matches!(self, Step::Verdict(v) if v.is_ok())
        }
    }
}
