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
//! | `release`        | `paymentInfo.payer`; the operator's declared lifecycle signer; the operator owner (`FEE_RECIPIENT()`, or `FEE_RECEIVER()` on v3 operators) |
//! | `refundInEscrow` | `paymentInfo.receiver`; the operator's declared lifecycle signer; the operator owner; the payer, but only once `authorizationExpiry` has passed (the chain already lets them `reclaim()`) |
//!
//! The receiver may never release (self-payment is what escrow exists to stop)
//! and the payer may never refund before expiry (that is the chargeback).
//!
//! The declared lifecycle signer (`declared_signer`) is the key
//! `OperatorAddresses::lifecycle_signers` names for `paymentInfo.operator` on
//! the order's network -- declared, rotated and revoked by pull request, and
//! announced in `/supported` (`lifecycleSigners`) from the same table. It is
//! checked locally, after the deadline, the signature and the nonce and before
//! the owner read, so an owner RPC that is down does not refuse it. It holds
//! the owner's power over every escrow of that operator: release to the
//! receiver, refund to the payer before expiry.
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

use super::abi::{OperatorContract, OperatorV3Contract};
use super::addresses::OperatorAddresses;
use super::errors::OperatorError;
use super::operator::OperatorAbi;
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
    /// The lifecycle signer the operator table declares for
    /// `paymentInfo.operator` on this network.
    DeclaredSigner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Payer => "payer",
            Role::Receiver => "receiver",
            Role::OperatorOwner => "operator_owner",
            Role::PayerAfterExpiry => "payer_after_expiry",
            Role::DeclaredSigner => "declared_signer",
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
    /// The lifecycle signer the operator table declares for
    /// `payment_info.operator` on `chain_id`'s network, if any
    /// ([`declared_signer`]).
    pub declared_signer: Option<Address>,
}

/// The first, chain-free half of the evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Settled without reading the chain.
    Verdict(Verdict),
    /// The signature is good and fresh but the signer is neither payer nor
    /// receiver (or holds no role as such) nor the declared lifecycle signer.
    /// Only the operator owner rule is left, and that one needs a read.
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
        // Deadline, signature and nonce are settled above; the owner read is
        // below. Local, so an owner RPC that is down cannot refuse it.
        None if ctx.declared_signer == Some(auth.signer) => Step::Verdict(Verdict::Ok {
            signer: auth.signer,
            role: Role::DeclaredSigner,
        }),
        None => Step::NeedsOwner {
            signer: auth.signer,
        },
    }
}

/// The lifecycle signer `table` declares for `operator` on `network`
/// (`OperatorAddresses::lifecycle_signer`). Production passes
/// `OperatorAddresses::for_network`.
pub fn declared_signer(
    table: impl Fn(Network) -> Option<OperatorAddresses>,
    network: Network,
    operator: Address,
) -> Option<Address> {
    table(network)?.lifecycle_signer(operator)
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
            format!(
                "{signer} is neither a party to this escrow, the operator's declared lifecycle \
                 signer, nor the operator owner"
            )
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

/// The owner of the operator: `FEE_RECIPIENT()` on the legacy and CREATE3
/// operators, `FEE_RECEIVER()` on v3 ones (which have no `FEE_RECIPIENT()`).
/// Immutable on every deployed operator, so it is read once per
/// (chain, operator) and cached for the process.
pub(super) async fn read_operator_owner(
    provider: &EvmProvider,
    chain_id: u64,
    operator: Address,
    abi: OperatorAbi,
) -> Result<Address, String> {
    if let Some(a) = owner_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(chain_id, operator))
    {
        return Ok(*a);
    }
    let owner: Address = match abi {
        OperatorAbi::V3 => {
            let call = OperatorV3Contract::FEE_RECEIVERCall {};
            let raw = super::operator::eth_call(provider, operator, &call)
                .await
                .map_err(|e| e.to_string())?;
            OperatorV3Contract::FEE_RECEIVERCall::abi_decode_returns(&raw)
                .map_err(|e| format!("decode FEE_RECEIVER: {e}"))?
        }
        OperatorAbi::Legacy | OperatorAbi::Create3 => {
            let call = OperatorContract::FEE_RECIPIENTCall {};
            let raw = super::operator::eth_call(provider, operator, &call)
                .await
                .map_err(|e| e.to_string())?;
            OperatorContract::FEE_RECIPIENTCall::abi_decode_returns(&raw)
                .map_err(|e| format!("decode FEE_RECIPIENT: {e}"))?
        }
    };
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
/// `abi` is the operator generation the request resolved to; it decides which
/// getter names the operator owner.
pub async fn gate(
    action: LifecycleAction,
    lifecycle: &EscrowLifecyclePayload,
    network: Network,
    provider: &EvmProvider,
    abi: OperatorAbi,
) -> Result<(), OperatorError> {
    gate_with_mode(mode(), action, lifecycle, network, provider, abi).await
}

/// [`gate`] under an explicit mode, so the chain half can be tested without
/// touching the process environment.
pub(super) async fn gate_with_mode(
    mode: Mode,
    action: LifecycleAction,
    lifecycle: &EscrowLifecyclePayload,
    network: Network,
    provider: &EvmProvider,
    abi: OperatorAbi,
) -> Result<(), OperatorError> {
    if mode == Mode::Off {
        return Ok(());
    }
    let payment_info = ContractPaymentInfo::from_lifecycle_payload(lifecycle);
    let verdict = evaluate(
        action,
        lifecycle,
        network,
        provider,
        abi,
        OperatorAddresses::for_network,
        unix_now(),
        replay_guard(),
    )
    .await?;
    let signer = match &verdict {
        Verdict::Ok { signer, .. }
        | Verdict::UnauthorizedRole { signer }
        | Verdict::OwnerUnverifiable { signer, .. } => Some(*signer),
        _ => lifecycle.lifecycle_auth.as_ref().map(|a| a.signer),
    };
    if let Verdict::Ok { role, .. } = &verdict {
        info!(
            action = action.as_str(),
            network = %network,
            mode = mode.as_str(),
            verdict = verdict.category(),
            role = role.as_str(),
            signer = ?signer,
            operator = ?payment_info.operator,
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
            operator = ?payment_info.operator,
            payer = ?payment_info.payer,
            receiver = ?payment_info.receiver,
            detail = %describe(&verdict),
            "escrow lifecycle order NOT authorized"
        );
    }
    decide(mode, &verdict)
}

/// The verdict on one order: the chain-free half ([`pre_evaluate`], with the
/// declared signer `table` names for `(network, paymentInfo.operator)`), then
/// the owner read only when that half leaves [`Step::NeedsOwner`]. Everything
/// [`gate_with_mode`] does short of logging and deciding; `table`, `now` and
/// `replay` are what it takes from the process (`OperatorAddresses::for_network`,
/// the clock, the process guard), so the policy vector can run the same path.
#[allow(clippy::too_many_arguments)]
pub(super) async fn evaluate(
    action: LifecycleAction,
    lifecycle: &EscrowLifecyclePayload,
    network: Network,
    provider: &EvmProvider,
    abi: OperatorAbi,
    table: impl Fn(Network) -> Option<OperatorAddresses>,
    now: u64,
    replay: &ReplayGuard,
) -> Result<Verdict, OperatorError> {
    let chain_id = EvmChain::try_from(network)
        .map_err(|_| OperatorError::NonEvmNetwork)?
        .chain_id;
    let payment_info = ContractPaymentInfo::from_lifecycle_payload(lifecycle);
    let ctx = OrderContext {
        action,
        payment_info: &payment_info,
        amount: lifecycle.amount,
        chain_id,
        now,
        max_deadline_secs: max_deadline_secs(),
        declared_signer: declared_signer(table, network, payment_info.operator),
    };
    Ok(
        match pre_evaluate(lifecycle.lifecycle_auth.as_ref(), &ctx, replay) {
            Step::Verdict(v) => v,
            Step::NeedsOwner { signer } => {
                let owner =
                    read_operator_owner(provider, chain_id, payment_info.operator, abi).await;
                finish_with_owner(signer, owner)
            }
        },
    )
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
            declared_signer: None,
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

    // ---- the owner getter follows the operator generation -----------------

    /// A release order signed by `owner` -- neither payer nor receiver, so only
    /// the operator-owner rule can admit it and the chain has to be read.
    fn owner_order(
        owner: &PrivateKeySigner,
        operator: Address,
        network: Network,
    ) -> EscrowLifecyclePayload {
        use crate::payment_operator::types::EscrowPaymentInfo;
        let now = unix_now();
        let mut lifecycle = EscrowLifecyclePayload {
            payment_info: EscrowPaymentInfo {
                operator,
                receiver: address!("2222222222222222222222222222222222222222"),
                token: address!("3600000000000000000000000000000000000000"),
                max_amount: 1_000_000,
                pre_approval_expiry: now + 3600,
                authorization_expiry: now + 7200,
                refund_expiry: now + 86400,
                min_fee_bps: 0,
                max_fee_bps: 1300,
                fee_receiver: operator,
                salt: B256::repeat_byte(0x39),
            },
            payer: address!("1111111111111111111111111111111111111111"),
            amount: 1_000_000,
            lifecycle_auth: None,
        };
        let chain_id = EvmChain::try_from(network).unwrap().chain_id;
        let deadline = now + 120;
        // Unique per operator, and every scenario below uses its own.
        let nonce = alloy::primitives::keccak256(operator.as_slice());
        let digest = signing_hash(
            LifecycleAction::Release,
            lifecycle.amount,
            deadline,
            nonce,
            &ContractPaymentInfo::from_lifecycle_payload(&lifecycle),
            chain_id,
        );
        lifecycle.lifecycle_auth = Some(LifecycleAuth {
            signer: owner.address(),
            deadline,
            nonce,
            signature: Bytes::from(owner.sign_hash_sync(&digest).unwrap().as_bytes().to_vec()),
        });
        lifecycle
    }

    #[tokio::test]
    async fn lifecycle_owner_reads_fee_receiver_on_v3() {
        use crate::payment_operator::test_rpc::{self, CallAnswer, MockNode};
        let owner = PrivateKeySigner::random();

        // v3 (Arc): FEE_RECEIVER() names the owner; FEE_RECIPIENT() does not
        // exist there, so the node reverts it.
        let node = MockNode::start(Network::Arc).await;
        let provider = test_rpc::provider(Network::Arc, &node, true).await;
        let operator = PrivateKeySigner::random().address();
        node.on_call(
            operator,
            OperatorV3Contract::FEE_RECEIVERCall::SELECTOR,
            CallAnswer::Return(alloy::sol_types::SolValue::abi_encode(&owner.address())),
        );
        let order = owner_order(&owner, operator, Network::Arc);
        gate_with_mode(
            Mode::Enforce,
            LifecycleAction::Release,
            &order,
            Network::Arc,
            &provider,
            OperatorAbi::V3,
        )
        .await
        .expect("the owner, read through FEE_RECEIVER(), may release");
        assert_eq!(
            node.reads(),
            vec![(operator, OperatorV3Contract::FEE_RECEIVERCall::SELECTOR)]
        );

        // A v3 owner read that fails is retryable (502 upstream_rpc_unavailable
        // in /settle), never a refusal of the owner.
        let unreadable = PrivateKeySigner::random().address();
        node.fail_reads(true);
        let err = gate_with_mode(
            Mode::Enforce,
            LifecycleAction::Release,
            &owner_order(&owner, unreadable, Network::Arc),
            Network::Arc,
            &provider,
            OperatorAbi::V3,
        )
        .await
        .expect_err("no verdict");
        assert!(
            matches!(
                err,
                OperatorError::LifecycleAuthRejected {
                    category: "owner_unverifiable",
                    ..
                }
            ),
            "{err:?}"
        );
        node.fail_reads(false);

        // The other generations keep FEE_RECIPIENT(), unchanged.
        let base = MockNode::start(Network::Base).await;
        let base_provider = test_rpc::provider(Network::Base, &base, true).await;
        let legacy = PrivateKeySigner::random().address();
        base.on_call(
            legacy,
            OperatorContract::FEE_RECIPIENTCall::SELECTOR,
            CallAnswer::Return(alloy::sol_types::SolValue::abi_encode(&owner.address())),
        );
        gate_with_mode(
            Mode::Enforce,
            LifecycleAction::Release,
            &owner_order(&owner, legacy, Network::Base),
            Network::Base,
            &base_provider,
            OperatorAbi::Legacy,
        )
        .await
        .expect("legacy owner through FEE_RECIPIENT()");
        assert_eq!(
            base.reads(),
            vec![(legacy, OperatorContract::FEE_RECIPIENTCall::SELECTOR)]
        );
    }

    // ---- the declared lifecycle signer: the shared policy vector ----------
    //
    // `tests/fixtures/escrow/lifecycle-declared-signer-vector.json`, loaded
    // byte for byte by Execution Market's lifecycle signer too. See
    // `super::super::lifecycle_vector`.

    use crate::payment_operator::addresses::{is_canonical_v1_network, LIFECYCLE_SIGNERS_VERSION};
    use crate::payment_operator::lifecycle_vector::{self, Case, Outcome, Vector};

    fn outcome_of_verdict(v: &Verdict) -> Outcome {
        match v {
            Verdict::Ok { role, .. } => Outcome {
                verdict: Some("ok".into()),
                role: Some(role.as_str().into()),
                step: None,
            },
            other => Outcome {
                verdict: Some(other.category().into()),
                role: None,
                step: None,
            },
        }
    }

    fn outcome_of_step(s: &Step) -> Outcome {
        match s {
            Step::Verdict(v) => outcome_of_verdict(v),
            Step::NeedsOwner { .. } => Outcome {
                verdict: None,
                role: None,
                step: Some("needs_owner".into()),
            },
        }
    }

    fn admits_declared_signer(o: &Outcome) -> bool {
        o.verdict.as_deref() == Some("ok") && o.role.as_deref() == Some("declared_signer")
    }

    /// The vector's keys are what their seeds derive, D is nobody else, and
    /// every digest and signature eth_account computed is the one this
    /// implementation computes: the declared role changes nothing in the
    /// EIP-712 order, and the signatures are byte for byte the RFC 6979 ones,
    /// so either side can be checked against the other's bytes.
    #[test]
    fn the_shared_vector_keys_digests_and_signatures_match_this_implementation() {
        let v = lifecycle_vector::vector();
        assert_eq!(v.vector_version, 1);
        assert_eq!(v.max_deadline_secs, DEFAULT_MAX_DEADLINE_SECS);
        assert_eq!(
            v.announcement.lifecycle_signers_version,
            LIFECYCLE_SIGNERS_VERSION
        );
        for (name, key) in &v.keys {
            assert_eq!(key.signer().address(), key.address, "key {name}");
            assert!(
                key.seed.contains("TEST KEY"),
                "key {name} is marked as a test key"
            );
        }
        let d = v.key("declaredSigner").address;
        for other in ["payer", "receiver", "owner", "otherKey"] {
            assert_ne!(d, v.key(other).address, "D is not the {other}");
        }
        assert!(v.cases.len() >= 20, "the whole matrix is there");
        for c in &v.cases {
            let payload = c.payload(&v);
            let pi = ContractPaymentInfo::from_lifecycle_payload(&payload);
            let signed_amount: u128 = c.signed_over.amount.parse().unwrap();
            let digest = signing_hash(
                c.action(),
                signed_amount,
                c.deadline,
                c.nonce(),
                &pi,
                c.signed_over.chain_id,
            );
            assert_eq!(digest, c.digest(), "{}: digest", c.id);
            let key = v.key(&c.signer);
            let ours = key.signer().sign_hash_sync(&digest).unwrap();
            assert_eq!(
                hex::encode(ours.as_bytes()),
                c.signature,
                "{}: signature",
                c.id
            );
            let auth = payload.lifecycle_auth.as_ref().unwrap();
            assert_eq!(recover(auth, digest), Some(key.address), "{}", c.id);
            // `emDelegate` says what the policy says about the declared role.
            match c.em_delegate {
                Some(true) => assert!(admits_declared_signer(&c.policy), "{}", c.id),
                Some(false) => {
                    assert_eq!(c.policy.step.as_deref(), Some("needs_owner"), "{}", c.id)
                }
                None => assert!(!admits_declared_signer(&c.policy), "{}", c.id),
            }
        }
    }

    /// The policy half, no chain: each case's first submission is the
    /// `policy` the vector states, and a resubmission is what `expected` says,
    /// with the declared signer resolved through [`declared_signer`] from the
    /// case's table.
    #[test]
    fn the_shared_vector_policy_matrix() {
        let v = lifecycle_vector::vector();
        for c in &v.cases {
            let payload = c.payload(&v);
            let pi = ContractPaymentInfo::from_lifecycle_payload(&payload);
            let network = c.network();
            let context = OrderContext {
                action: c.action(),
                payment_info: &pi,
                amount: c.amount(),
                chain_id: EvmChain::try_from(network).unwrap().chain_id,
                now: v.now,
                max_deadline_secs: v.max_deadline_secs,
                declared_signer: declared_signer(v.table(&c.table), network, pi.operator),
            };
            let guard = ReplayGuard::default();
            let auth = payload.lifecycle_auth.as_ref();
            let first = pre_evaluate(auth, &context, &guard);
            assert_eq!(outcome_of_step(&first), c.policy, "{}", c.id);
            for (i, expected) in c.expected.iter().enumerate().skip(1) {
                let again = pre_evaluate(auth, &context, &guard);
                assert_eq!(&outcome_of_step(&again), expected, "{} #{i}", c.id);
            }
        }
    }

    async fn through_the_gate(v: &Vector, c: &Case) -> (Vec<Outcome>, Vec<(Address, [u8; 4])>) {
        use crate::payment_operator::test_rpc::{self, CallAnswer, MockNode};
        let network = c.network();
        let (abi, selector) = if is_canonical_v1_network(network) {
            (
                OperatorAbi::V3,
                OperatorV3Contract::FEE_RECEIVERCall::SELECTOR,
            )
        } else {
            (
                OperatorAbi::Legacy,
                OperatorContract::FEE_RECIPIENTCall::SELECTOR,
            )
        };
        // A cached owner would answer without a read: every case starts cold,
        // so "no owner read" is measured, not inherited from an earlier case.
        owner_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let node = MockNode::start(network).await;
        let provider = test_rpc::provider(network, &node, true).await;
        match c.owner_rpc.as_str() {
            "up" => node.on_call(
                c.payment_info.operator,
                selector,
                CallAnswer::Return(alloy::sol_types::SolValue::abi_encode(
                    &v.key("owner").address,
                )),
            ),
            "down" => node.fail_reads(true),
            other => panic!("{}: ownerRpc {other}", c.id),
        }
        let payload = c.payload(v);
        let guard = ReplayGuard::default();
        let mut outcomes = Vec::new();
        for _ in &c.expected {
            let verdict = evaluate(
                c.action(),
                &payload,
                network,
                &provider,
                abi,
                v.table(&c.table),
                v.now,
                &guard,
            )
            .await
            .expect("an EVM network");
            outcomes.push(outcome_of_verdict(&verdict));
        }
        (outcomes, node.reads())
    }

    /// The whole matrix through the path `gate` runs: the declared signer
    /// resolved from the case's table, the policy, and the owner read only
    /// when the policy leaves it. A declared signer is admitted with the owner
    /// RPC down and without a single owner read; anything the declared rule
    /// does not admit still ends where it ended before it existed.
    #[tokio::test]
    async fn the_shared_vector_through_the_gate() {
        let v = lifecycle_vector::vector();
        for c in &v.cases {
            let (outcomes, reads) = through_the_gate(&v, c).await;
            assert_eq!(outcomes, c.expected, "{}", c.id);
            if c.policy.step.is_none() {
                assert!(
                    reads.is_empty(),
                    "{}: settled locally, read {reads:?}",
                    c.id
                );
            }
        }
    }

    /// The production gate reads the production table, which declares nobody:
    /// an order the vector's D signs for Base/A -- admitted when the vector's
    /// table declares D -- is refused under `enforce` exactly as before the
    /// role existed, through the owner rule.
    #[tokio::test]
    async fn the_production_gate_admits_no_declared_signer_while_the_table_is_empty() {
        use crate::payment_operator::test_rpc::{self, CallAnswer, MockNode};
        let v = lifecycle_vector::vector();
        let d = v.key("declaredSigner").signer();
        let base_a = address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb");
        owner_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let node = MockNode::start(Network::Base).await;
        let provider = test_rpc::provider(Network::Base, &node, true).await;
        node.on_call(
            base_a,
            OperatorContract::FEE_RECIPIENTCall::SELECTOR,
            CallAnswer::Return(alloy::sol_types::SolValue::abi_encode(
                &v.key("owner").address,
            )),
        );
        let order = owner_order(&d, base_a, Network::Base);
        let err = gate_with_mode(
            Mode::Enforce,
            LifecycleAction::Release,
            &order,
            Network::Base,
            &provider,
            OperatorAbi::Legacy,
        )
        .await
        .expect_err("nobody is declared");
        assert!(
            matches!(
                err,
                OperatorError::LifecycleAuthRejected {
                    category: "unauthorized_role",
                    ..
                }
            ),
            "{err:?}"
        );
        assert_eq!(
            node.reads(),
            vec![(base_a, OperatorContract::FEE_RECIPIENTCall::SELECTOR)]
        );
    }

    /// A declared signer is admitted only for the operator AND the network
    /// it is declared for; existing roles keep their own label even when the
    /// same key is also declared.
    #[test]
    fn a_declared_signer_is_bound_to_its_operator_and_network() {
        let v = lifecycle_vector::vector();
        let d = v.key("declaredSigner").address;
        let table = v.table("declared");
        let base_a = address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb");
        let base_b = address!("030353642B936c9D4213caD7BcB0fB8a1489cBe5");
        let shared = address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e");
        assert_eq!(declared_signer(&table, Network::Base, base_a), Some(d));
        assert_eq!(declared_signer(&table, Network::Base, base_b), None);
        assert_eq!(declared_signer(&table, Network::Arbitrum, shared), Some(d));
        assert_eq!(declared_signer(&table, Network::Celo, shared), None);
        assert_eq!(declared_signer(&table, Network::Solana, base_a), None);
        // The real table declares nobody, for any operator of any network.
        for &network in crate::payment_operator::addresses::ESCROW_NETWORKS {
            for operator in OperatorAddresses::for_network(network)
                .unwrap()
                .payment_operators
            {
                assert_eq!(
                    declared_signer(OperatorAddresses::for_network, network, operator),
                    None,
                    "{network}"
                );
            }
        }

        // D also the payer: a payer release stays `payer`.
        let payer = v.key("declaredSigner").signer();
        let pi = payment_info(payer.address(), Address::ZERO, NOW + 3600);
        let mut c = ctx(LifecycleAction::Release, &pi);
        c.declared_signer = Some(payer.address());
        let auth = sign(&payer, &c, NOW + 60, nonce(1));
        assert_eq!(
            pre_evaluate(Some(&auth), &c, &ReplayGuard::default()),
            Step::Verdict(Verdict::Ok {
                signer: payer.address(),
                role: Role::Payer
            })
        );
        // The same key on a refund before expiry, where the payer has no role:
        // admitted as the declared signer, without the owner rule.
        let mut r = ctx(LifecycleAction::RefundInEscrow, &pi);
        r.declared_signer = Some(payer.address());
        let auth = sign(&payer, &r, NOW + 60, nonce(2));
        assert_eq!(
            pre_evaluate(Some(&auth), &r, &ReplayGuard::default()),
            Step::Verdict(Verdict::Ok {
                signer: payer.address(),
                role: Role::DeclaredSigner
            })
        );
        assert_eq!(Role::DeclaredSigner.as_str(), "declared_signer");
        assert_eq!(
            describe(&Verdict::Ok {
                signer: payer.address(),
                role: Role::DeclaredSigner
            }),
            format!("signed by {} as declared_signer", payer.address())
        );
    }
}
