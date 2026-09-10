//! Minting an ERC-8004 identity on Solana, atomically.
//!
//! # Why this module exists
//!
//! Handing an agent its identity is three registry instructions, and until
//! v2.17.0 they went out as three separate transactions:
//!
//! ```text
//! Register | CreateV2   creates the Core asset + the agent PDA, owner = facilitator
//! InitializeStats       creates the ATOM stats PDA (only the OWNER may do this)
//! TransferAgent         hands the asset to the agent
//! ```
//!
//! A transaction that lands is final on its own, so any prefix of that sequence
//! is a reachable end state. When the fee payer ran out of SOL mid-batch on
//! 2026-09-09, two of KarmaKadabra's twenty agents ended at the first step: the
//! account exists, the facilitator owns it, and `GET /identity/solana/owner/<pubkey>`
//! answers 404 because the agent is genuinely not the owner. Retrying minted a
//! *second* half-identity, because nothing linked the retry to the first attempt.
//! Four assets are stranded in the facilitator's name that way.
//!
//! Three things follow, and this module is all three:
//!
//! 1. **One transaction.** All the instructions ride in a single transaction
//!    whenever the wire size and the compute budget allow it, so there is no
//!    prefix to get stuck on. [`plan_mint`] measures both and says when it does
//!    not fit; the caller then stages the sends and reports where it stopped.
//! 2. **A retry resumes.** [`decide_mint`] recognises a half-minted identity by
//!    its `agent_uri` among the assets the fee payer still holds, so the second
//!    call finishes the first one instead of creating another orphan.
//! 3. **A balance check before the send.** [`estimate_mint_cost`] prices the mint
//!    from on-chain rent rather than from a guess, so an underfunded fee payer
//!    is a named 503 instead of an RPC `-32002 Transaction simulation failed`
//!    that reads like a bug in the program.
//!
//! Everything here that decides something is a pure function over values, and is
//! tested as one. The RPC round trips live in `handlers.rs`, which is the only
//! part that needs a chain.

use serde::{Deserialize, Serialize};
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::transaction::Transaction;

use crate::erc8004::solana::{
    build_initialize_stats_ix, build_register_ix, build_set_metadata_pda_ix,
    build_transfer_agent_ix, AgentAccount, RegistryContext, SolanaErc8004Programs,
};
use crate::types::TransactionHash;

// ============================================================================
// Chain limits
// ============================================================================

/// Largest a serialized transaction may be on the wire (`PACKET_DATA_SIZE`).
///
/// Hard-coded rather than imported so the bound this module reasons about is
/// visible next to the reasoning, and so the size test pins a number rather
/// than restating whatever the SDK happens to export.
pub const PACKET_DATA_SIZE: usize = 1232;

/// Compute units the runtime grants per instruction when a transaction carries
/// no `ComputeBudget` instruction of its own.
pub const DEFAULT_COMPUTE_UNITS_PER_INSTRUCTION: u32 = 200_000;

/// Ceiling on the whole transaction's default compute budget.
pub const MAX_COMPUTE_UNITS_PER_TRANSACTION: u32 = 1_400_000;

/// Instructions a bundle may hold before the default budget stops being
/// `200_000` per instruction.
///
/// This is the load-bearing reason bundling is safe on compute: today each of
/// the three instructions runs *alone* in its own transaction with a 200 000 CU
/// budget and succeeds. Bundled, a transaction of `n` instructions is granted
/// `min(200_000 * n, 1_400_000)`, so as long as `n <= 7` every instruction keeps
/// at least the budget it already has. Past seven the ceiling starts diluting
/// them and the argument no longer holds, so the bundle is split instead.
pub const MAX_BUNDLED_INSTRUCTIONS: usize =
    (MAX_COMPUTE_UNITS_PER_TRANSACTION / DEFAULT_COMPUTE_UNITS_PER_INSTRUCTION) as usize;

/// Lamports the runtime charges per signature (`DEFAULT_LAMPORTS_PER_SIGNATURE`).
pub const LAMPORTS_PER_SIGNATURE: u64 = 5_000;

/// One SOL, in lamports. Used only to render costs in log lines and errors.
pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

// ============================================================================
// What a mint costs
// ============================================================================

/// Bytes the registry allocates for an `AgentAccount`, discriminator included.
///
/// The account is sized for maximum-length strings, so it does not shrink with a
/// short `agent_uri`. Over-estimating here only makes the preflight stricter: it
/// can refuse a mint that would have squeaked through, never allow one that runs
/// the fee payer dry.
pub const AGENT_ACCOUNT_ALLOC_BYTES: usize = 748;

/// Bytes the ATOM Engine allocates for an `AtomStats` account, discriminator
/// included. Pinned by `test_atom_stats_is_561_bytes` in `solana.rs`.
pub const ATOM_STATS_ALLOC_BYTES: usize = 561;

/// Lamports the Metaplex Core asset account costs to create.
///
/// mpl-core sizes the asset itself and the registry does not expose the number,
/// so unlike the two PDAs above this one cannot be computed from `Rent`. The
/// default is the difference between KarmaKadabra's measured register-transaction
/// cost (0.0044-0.0089 SOL, 2026-09-09) and the rent this module computes for the
/// agent PDA, rounded up. Override with `X402_SOLANA_MINT_ASSET_LAMPORTS`.
pub const DEFAULT_ASSET_RENT_LAMPORTS: u64 = 2_500_000;

/// Env var overriding [`DEFAULT_ASSET_RENT_LAMPORTS`].
pub const ENV_ASSET_RENT_LAMPORTS: &str = "X402_SOLANA_MINT_ASSET_LAMPORTS";

/// Env var overriding how many mints of headroom [`MintCost::mints_remaining`]
/// reports against, and therefore what the balance log line calls "low".
pub const ENV_MINT_HEADROOM: &str = "X402_SOLANA_MINT_HEADROOM";

/// Mints of headroom the facilitator wants to keep in the fee payer.
///
/// Not a gate -- a mint is refused only when it cannot be paid for at all. This
/// is the number the balance log line and the CloudWatch alarm are sized on.
pub const DEFAULT_MINT_HEADROOM: u64 = 50;

/// What one mint will cost the fee payer, split so a log line can say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MintCost {
    /// Rent-exemption deposits for every account the mint creates.
    pub rent_lamports: u64,
    /// Signature fees across every transaction the mint sends.
    pub fee_lamports: u64,
    /// Transactions the mint will send.
    pub transactions: usize,
    /// Signatures across all of them.
    pub signatures: usize,
}

impl MintCost {
    /// What the fee payer must hold for this mint to go through.
    pub fn total_lamports(&self) -> u64 {
        self.rent_lamports.saturating_add(self.fee_lamports)
    }

    /// How many more mints a balance covers. Saturates at zero.
    pub fn mints_remaining(&self, available_lamports: u64) -> u64 {
        let total = self.total_lamports();
        if total == 0 {
            return u64::MAX;
        }
        available_lamports / total
    }
}

/// Read a `u64` env override, falling back to `default` when unset or unparseable.
fn env_u64(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Lamports the Metaplex Core asset costs, honouring [`ENV_ASSET_RENT_LAMPORTS`].
pub fn asset_rent_lamports() -> u64 {
    env_u64(ENV_ASSET_RENT_LAMPORTS, DEFAULT_ASSET_RENT_LAMPORTS)
}

/// Mints of headroom to keep, honouring [`ENV_MINT_HEADROOM`].
pub fn mint_headroom() -> u64 {
    env_u64(ENV_MINT_HEADROOM, DEFAULT_MINT_HEADROOM)
}

/// Bytes a `MetadataEntryPda` occupies for a given key and value.
///
/// disc(8) + asset(32) + immutable(1) + bump(1) + borsh string(4 + key) +
/// borsh bytes(4 + value).
pub fn metadata_account_len(key: &str, value_len: usize) -> usize {
    8 + 32 + 1 + 1 + 4 + key.len() + 4 + value_len
}

/// Price a mint from on-chain rent rather than from a headline number.
///
/// `creates_asset` is false on a resume, where the Core asset and the agent PDA
/// already exist and only the remaining steps are paid for.
pub fn estimate_mint_cost(
    creates_asset: bool,
    creates_stats: bool,
    metadata: &[(String, usize)],
    transactions: usize,
    signatures: usize,
) -> MintCost {
    let rent = Rent::default();
    let mut rent_lamports = 0u64;

    if creates_asset {
        rent_lamports = rent_lamports
            .saturating_add(rent.minimum_balance(AGENT_ACCOUNT_ALLOC_BYTES))
            .saturating_add(asset_rent_lamports());
    }
    if creates_stats {
        rent_lamports = rent_lamports.saturating_add(rent.minimum_balance(ATOM_STATS_ALLOC_BYTES));
    }
    for (key, value_len) in metadata {
        rent_lamports = rent_lamports
            .saturating_add(rent.minimum_balance(metadata_account_len(key, *value_len)));
    }

    MintCost {
        rent_lamports,
        fee_lamports: LAMPORTS_PER_SIGNATURE.saturating_mul(signatures as u64),
        transactions,
        signatures,
    }
}

/// Render lamports as SOL with nine decimals, for log lines and error text.
pub fn lamports_to_sol_string(lamports: u64) -> String {
    format!(
        "{}.{:09}",
        lamports / LAMPORTS_PER_SOL,
        lamports % LAMPORTS_PER_SOL
    )
}

// ============================================================================
// Planning the transaction
// ============================================================================

/// Why a mint could not go out as one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAtomic {
    /// The signed transaction would not fit in one packet.
    TooLarge { serialized_len: usize, limit: usize },
    /// Enough instructions that the default compute budget stops granting each
    /// of them the 200 000 CU it gets when it runs alone.
    ComputeDiluted { instructions: usize, limit: usize },
}

impl std::fmt::Display for NotAtomic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotAtomic::TooLarge {
                serialized_len,
                limit,
            } => write!(
                f,
                "transaction would serialize to {serialized_len} bytes, over the {limit}-byte packet limit"
            ),
            NotAtomic::ComputeDiluted {
                instructions,
                limit,
            } => write!(
                f,
                "{instructions} instructions exceed the {limit} that keep a full 200,000 CU each"
            ),
        }
    }
}

/// Which of the mint's steps an instruction is.
///
/// Carried alongside the instructions so the staged fallback can name the step
/// that failed instead of reporting an index, and so the response can say which
/// signature belongs to which step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintStep {
    /// `register` -- creates the Core asset and the agent PDA.
    Register,
    /// `set_metadata_pda` -- one per entry the caller asked for.
    Metadata,
    /// `initialize_stats` -- creates the ATOM stats account. Owner-only.
    InitializeStats,
    /// `transfer_agent` -- hands the identity to the recipient. Owner-only, and
    /// last, because it ends the facilitator's ownership.
    Transfer,
}

/// The instructions a mint needs, and whether they fit in one transaction.
#[derive(Debug, Clone)]
pub struct MintPlan {
    /// In execution order. `initialize_stats` always precedes `transfer_agent`:
    /// only the owner may initialize, and after the transfer the facilitator is
    /// not the owner any more.
    pub instructions: Vec<Instruction>,
    /// What each instruction is, index for index with `instructions`.
    pub steps: Vec<MintStep>,
    /// Wire size of the signed transaction if all of it goes out at once.
    pub serialized_len: usize,
    /// Compute units the runtime would grant that transaction.
    pub compute_units: u32,
    /// `None` when the plan is one transaction.
    pub not_atomic: Option<NotAtomic>,
    /// True when the plan mints a new asset (as opposed to resuming one).
    pub creates_asset: bool,
    /// True when the plan creates the ATOM stats account.
    pub creates_stats: bool,
    /// True when the plan ends with a transfer to a recipient.
    pub transfers: bool,
}

impl MintPlan {
    /// Whether the whole mint can go out as a single transaction.
    pub fn is_atomic(&self) -> bool {
        self.not_atomic.is_none()
    }
}

/// Size of the signed transaction these instructions would produce.
///
/// Signatures are fixed-width, so an unsigned transaction serializes to exactly
/// the same length as the signed one and no keypair is needed to measure.
pub fn signed_transaction_size(instructions: &[Instruction], fee_payer: &Pubkey) -> usize {
    let message = Message::new(instructions, Some(fee_payer));
    let tx = Transaction::new_unsigned(message);
    bincode::serialize(&tx)
        .map(|b| b.len())
        .unwrap_or(usize::MAX)
}

/// Compute units the runtime grants a transaction with `n` instructions and no
/// `ComputeBudget` instruction of its own.
pub fn default_compute_units(instructions: usize) -> u32 {
    DEFAULT_COMPUTE_UNITS_PER_INSTRUCTION
        .saturating_mul(instructions as u32)
        .min(MAX_COMPUTE_UNITS_PER_TRANSACTION)
}

/// What a caller is asking the facilitator to mint.
pub struct MintRequest<'a> {
    pub programs: &'a SolanaErc8004Programs,
    pub registry: &'a RegistryContext,
    /// The Core asset: a fresh keypair on a new mint, the stranded asset on a resume.
    pub asset: &'a Pubkey,
    /// The facilitator's fee payer, which is also the owner every instruction signs as.
    pub fee_payer: &'a Pubkey,
    pub agent_uri: &'a str,
    /// Key and raw value bytes for each `set_metadata_pda` entry.
    pub metadata: &'a [(String, Vec<u8>)],
    pub recipient: Option<&'a Pubkey>,
}

/// Build the instruction sequence for a fresh mint and measure it.
pub fn plan_mint(request: &MintRequest<'_>) -> MintPlan {
    let mut instructions = vec![build_register_ix(
        request.programs,
        request.registry,
        request.asset,
        request.fee_payer,
        request.agent_uri,
    )];
    let mut steps = vec![MintStep::Register];

    // Metadata rides ahead of the transfer for the same reason the stats do:
    // `set_metadata_pda` is signed by the owner, and after the transfer that is
    // no longer us.
    //
    // It is part of the atomic unit rather than the best-effort afterthought it
    // used to be. An entry the program refuses now fails the whole mint with
    // nothing left on chain, where before it produced a warning and an identity
    // that silently lacked the metadata it was registered with -- the same shape
    // of half-success this module exists to remove.
    for (key, value) in request.metadata {
        instructions.push(build_set_metadata_pda_ix(
            request.programs,
            request.asset,
            request.fee_payer,
            key,
            value,
            false,
        ));
        steps.push(MintStep::Metadata);
    }

    instructions.push(build_initialize_stats_ix(
        request.programs,
        &request.registry.collection,
        request.asset,
        request.fee_payer,
    ));
    steps.push(MintStep::InitializeStats);

    plan_from_instructions(request, instructions, steps, true, true)
}

/// Build the instruction sequence that finishes a half-minted identity.
///
/// Metadata is deliberately NOT re-issued here. `set_metadata_pda` creates the
/// entry's PDA, so replaying it against an entry the first attempt already wrote
/// would fail the whole transaction and turn a recoverable orphan into a
/// permanent one. The identity's owner can set metadata once it is theirs.
pub fn plan_resume(request: &MintRequest<'_>, needs_stats: bool) -> MintPlan {
    let mut instructions = Vec::new();
    let mut steps = Vec::new();
    if needs_stats {
        instructions.push(build_initialize_stats_ix(
            request.programs,
            &request.registry.collection,
            request.asset,
            request.fee_payer,
        ));
        steps.push(MintStep::InitializeStats);
    }
    plan_from_instructions(request, instructions, steps, false, needs_stats)
}

fn plan_from_instructions(
    request: &MintRequest<'_>,
    mut instructions: Vec<Instruction>,
    mut steps: Vec<MintStep>,
    creates_asset: bool,
    creates_stats: bool,
) -> MintPlan {
    let transfers = request.recipient.is_some();
    if let Some(recipient) = request.recipient {
        instructions.push(build_transfer_agent_ix(
            request.programs,
            &request.registry.collection,
            request.asset,
            request.fee_payer,
            recipient,
        ));
        steps.push(MintStep::Transfer);
    }

    let serialized_len = signed_transaction_size(&instructions, request.fee_payer);
    let compute_units = default_compute_units(instructions.len());

    let not_atomic = if instructions.len() > MAX_BUNDLED_INSTRUCTIONS {
        Some(NotAtomic::ComputeDiluted {
            instructions: instructions.len(),
            limit: MAX_BUNDLED_INSTRUCTIONS,
        })
    } else if serialized_len > PACKET_DATA_SIZE {
        Some(NotAtomic::TooLarge {
            serialized_len,
            limit: PACKET_DATA_SIZE,
        })
    } else {
        None
    };

    MintPlan {
        instructions,
        steps,
        serialized_len,
        compute_units,
        not_atomic,
        creates_asset,
        creates_stats,
        transfers,
    }
}

// ============================================================================
// Deciding whether to mint or to resume
// ============================================================================

/// What a `/register` call should do, given what the fee payer already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintDecision {
    /// Nothing on chain matches this request: mint a new identity.
    Fresh,
    /// A half-minted identity for this `agent_uri` is still in the facilitator's
    /// name. Finish that one rather than create another orphan.
    Resume { asset: Pubkey },
}

/// Decide between minting and resuming.
///
/// `facilitator_held` is what `find_agents_by_owner(fee_payer)` returned, so
/// every entry is by construction an asset the facilitator still owns -- either
/// one that stalled before its transfer, or one deliberately registered without
/// a recipient. Both are unreachable to anybody else, so adopting either is
/// safe; the `agent_uri` is what ties one to this request.
///
/// An empty `agent_uri` never matches. It is `#[serde(default)]` on the request,
/// so treating it as an identity key would let one URI-less call adopt an
/// unrelated URI-less asset.
pub fn decide_mint(
    agent_uri: &str,
    fee_payer: &Pubkey,
    facilitator_held: &[(Pubkey, AgentAccount)],
) -> MintDecision {
    if agent_uri.is_empty() {
        return MintDecision::Fresh;
    }
    for (_, agent) in facilitator_held {
        if agent.agent_uri == agent_uri && agent.owner == fee_payer.to_bytes() {
            return MintDecision::Resume {
                asset: Pubkey::new_from_array(agent.asset),
            };
        }
    }
    MintDecision::Fresh
}

// ============================================================================
// Reporting what actually happened
// ============================================================================

/// How far the mint got on chain.
///
/// The point of the enum is that `success` alone cannot say this. Before
/// v2.17.0 a mint that stopped after `Register` still answered with an
/// `agentId`, and the caller had no field to tell it apart from a delivered
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MintStatus {
    /// No identity exists as a result of this request, and nothing was left
    /// behind. Either the request was refused before it touched the chain, or
    /// the single transaction carrying the whole mint reverted. Retrying is
    /// safe: there is no stranded asset to reclaim.
    NotMinted,
    /// The identity exists but has no ATOM stats account, and the facilitator
    /// still owns it. Retry to finish it; feedback would not score as it stands.
    PendingStats,
    /// The identity exists with its stats, and the facilitator still owns it.
    /// Retry to hand it over.
    PendingTransfer,
    /// Everything the request asked for confirmed.
    Complete,
}

impl MintStatus {
    /// Whether this is an end state the caller can rely on.
    pub fn is_complete(&self) -> bool {
        matches!(self, MintStatus::Complete)
    }
}

/// Machine-readable reasons a mint was refused or stopped short.
pub const ERROR_FEE_PAYER_INSUFFICIENT_BALANCE: &str = "fee_payer_insufficient_balance";
pub const ERROR_OWNER_LOOKUP_INCONCLUSIVE: &str = "owner_lookup_inconclusive";
pub const ERROR_REGISTER_FAILED: &str = "register_failed";
pub const ERROR_STATS_FAILED: &str = "initialize_stats_failed";
pub const ERROR_TRANSFER_FAILED: &str = "transfer_failed";
pub const ERROR_METADATA_FAILED: &str = "set_metadata_failed";
/// The single transaction carrying the whole mint reverted. Which instruction
/// tripped it is not reported, because the only way to know from
/// `send_and_confirm_transaction` is to parse "Error processing Instruction N"
/// out of the RPC's prose, and a code derived from a substring match is worse
/// than no code. Nothing landed either way -- that is what atomic means -- and
/// `status` already distinguishes a mint that left nothing behind from a resume
/// that found the identity where it was.
pub const ERROR_MINT_TRANSACTION_FAILED: &str = "mint_transaction_failed";
pub const ERROR_STATS_LOOKUP_INCONCLUSIVE: &str = "stats_lookup_inconclusive";

/// The `ERROR_*` code for a step that failed.
pub fn error_code_for(step: MintStep) -> &'static str {
    match step {
        MintStep::Register => ERROR_REGISTER_FAILED,
        MintStep::Metadata => ERROR_METADATA_FAILED,
        MintStep::InitializeStats => ERROR_STATS_FAILED,
        MintStep::Transfer => ERROR_TRANSFER_FAILED,
    }
}

/// What the fee payer holds against what this mint needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeePayerFunds {
    /// The facilitator's Solana fee payer, base58.
    pub address: String,
    /// Balance read immediately before the send.
    pub available_lamports: u64,
    /// What this mint needs: rent for every account it creates, plus fees.
    pub required_lamports: u64,
    /// Mints the balance still covers at that price.
    pub mints_remaining: u64,
}

impl FeePayerFunds {
    pub fn new(address: &Pubkey, available_lamports: u64, cost: &MintCost) -> Self {
        Self {
            address: address.to_string(),
            available_lamports,
            required_lamports: cost.total_lamports(),
            mints_remaining: cost.mints_remaining(available_lamports),
        }
    }

    /// Whether the balance covers the mint.
    pub fn is_sufficient(&self) -> bool {
        self.available_lamports >= self.required_lamports
    }
}

/// The Solana-only detail of a `POST /register` response.
///
/// Carried under `mint` so the shared [`crate::erc8004::RegisterAgentResponse`]
/// contract that EVM clients read is untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolanaMintReport {
    /// How far the mint got. Read this, not `success`, to decide whether to retry.
    pub status: MintStatus,
    /// Why it stopped, when it stopped. One of the `ERROR_*` constants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// The `initialize_stats` signature. Equal to `transaction` when the mint
    /// went out atomically, because one transaction carried every instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats_transaction: Option<TransactionHash>,
    /// True when this call finished an identity an earlier call had left half
    /// minted, rather than creating a new one.
    pub resumed: bool,
    /// True when every instruction rode in one transaction, so no prefix of the
    /// mint could land on its own.
    pub atomic: bool,
    /// True when this call carried metadata that it did not write, which happens
    /// only on a resume: replaying `set_metadata_pda` against an entry the first
    /// attempt already created would fail the transaction and strand the
    /// identity for good. The owner can set it once the identity is theirs.
    pub metadata_skipped: bool,
    /// Present whenever the fee payer's balance was read, which is every request
    /// that reached the chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_payer: Option<FeePayerFunds>,
}

/// Everything a mint attempt learned, before it is shaped into a response.
#[derive(Debug, Clone)]
pub struct MintOutcome {
    /// The Core asset, which is the Solana agent ID.
    pub asset: Pubkey,
    /// The facilitator's fee payer.
    pub fee_payer: Pubkey,
    /// Who the caller asked us to hand the identity to, if anyone.
    pub recipient: Option<Pubkey>,
    /// Whether the identity exists on chain now: this call created it, or it
    /// already existed and this call resumed it. Distinct from `register_tx`,
    /// which is only set when *this* call did the creating.
    pub registered: bool,
    /// Signature of the transaction that created the identity, when this call
    /// created it.
    pub register_tx: Option<TransactionHash>,
    /// Signature that created the ATOM stats account.
    pub stats_tx: Option<TransactionHash>,
    /// Signature that handed the identity over.
    pub transfer_tx: Option<TransactionHash>,
    /// Whether the ATOM stats account exists now.
    pub stats_ready: bool,
    /// Whether this call adopted a half-minted identity.
    pub resumed: bool,
    /// Whether metadata the caller sent was not written. See
    /// [`SolanaMintReport::metadata_skipped`].
    pub metadata_skipped: bool,
    /// Whether the whole mint went out as one transaction.
    pub atomic: bool,
    /// Fee payer funds as read before the send.
    pub funds: Option<FeePayerFunds>,
    /// Set when a step failed: its `ERROR_*` code and the RPC's own words.
    pub failure: Option<(String, String)>,
}

impl MintOutcome {
    /// How far the mint got, read off what actually confirmed.
    ///
    /// The transfer is the last gate: while the facilitator still holds the
    /// asset, the caller's agent does not have an identity no matter what the
    /// registry contains. When no recipient was asked for, the facilitator
    /// holding it *is* the requested end state.
    pub fn status(&self) -> MintStatus {
        if !self.registered {
            return MintStatus::NotMinted;
        }
        if !self.stats_ready {
            return MintStatus::PendingStats;
        }
        match self.recipient {
            Some(_) if self.transfer_tx.is_none() => MintStatus::PendingTransfer,
            _ => MintStatus::Complete,
        }
    }

    /// Who owns the identity now.
    pub fn owner(&self) -> Pubkey {
        match (self.recipient, &self.transfer_tx) {
            (Some(recipient), Some(_)) => recipient,
            _ => self.fee_payer,
        }
    }

    /// The Solana half of the `/register` response body.
    pub fn report(&self) -> SolanaMintReport {
        SolanaMintReport {
            status: self.status(),
            error_code: self.failure.as_ref().map(|(code, _)| code.clone()),
            stats_transaction: self.stats_tx.clone(),
            resumed: self.resumed,
            atomic: self.atomic,
            metadata_skipped: self.metadata_skipped,
            fee_payer: self.funds.clone(),
        }
    }

    /// Human-readable failure text, naming what the operator has to do next.
    ///
    /// A pending mint is not a lost one: the asset is in the facilitator's name
    /// and the same request completes it, so the message says so rather than
    /// leaving the caller to guess whether to retry.
    pub fn error_message(&self) -> Option<String> {
        let (_, detail) = self.failure.as_ref()?;
        Some(match self.status() {
            MintStatus::PendingStats => format!(
                "Identity {} exists but its ATOM stats were not initialized and it is still \
                 held by the facilitator. Repeat this same request to finish it; it will \
                 resume this identity rather than mint another. Cause: {}",
                self.asset, detail
            ),
            MintStatus::PendingTransfer => format!(
                "Identity {} exists and is initialized but is still held by the facilitator. \
                 Repeat this same request to finish the transfer; it will resume this identity \
                 rather than mint another. Cause: {}",
                self.asset, detail
            ),
            _ => detail.clone(),
        })
    }
}

/// A refusal that never touched the chain.
pub fn refusal(
    fee_payer: Pubkey,
    error_code: &str,
    detail: String,
    funds: Option<FeePayerFunds>,
) -> MintOutcome {
    MintOutcome {
        asset: Pubkey::default(),
        fee_payer,
        recipient: None,
        registered: false,
        register_tx: None,
        stats_tx: None,
        transfer_tx: None,
        stats_ready: false,
        resumed: false,
        metadata_skipped: false,
        atomic: false,
        funds,
        failure: Some((error_code.to_string(), detail)),
    }
}

/// The state a mint attempt starts in, before anything is sent.
pub fn attempt(
    asset: Pubkey,
    fee_payer: Pubkey,
    recipient: Option<Pubkey>,
    plan: &MintPlan,
    resumed: bool,
    metadata_skipped: bool,
    funds: Option<FeePayerFunds>,
) -> MintOutcome {
    MintOutcome {
        asset,
        fee_payer,
        recipient,
        // A resume starts with the identity already on chain; a fresh mint has
        // to earn that bit by landing its `register`.
        registered: resumed,
        register_tx: None,
        stats_tx: None,
        transfer_tx: None,
        // Likewise the stats: a resume that found them does not re-create them.
        stats_ready: resumed && !plan.creates_stats,
        resumed,
        metadata_skipped,
        atomic: plan.is_atomic(),
        funds,
        failure: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erc8004::solana::{derive_agent_pda, get_program_ids};
    use crate::network::Network;
    use std::str::FromStr;

    /// Base collection held by RootConfig on mainnet (verified on-chain 2026-08-07).
    const MAINNET_COLLECTION: &str = "DbjsWo7iUs7QZyJxLgNyVxvAAjQZCXroJHoGok8h8Umg";

    /// The facilitator's Solana mainnet fee payer, from `lambda/balances/handler.py`.
    /// It is the owner stamped on all four of KarmaKadabra's stranded assets.
    const FACILITATOR_FEE_PAYER: &str = "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq";

    /// `kk-0xyuls`'s two half-minted assets (2026-09-09).
    const ORPHAN_YULS_A: &str = "AjANABVeKCfVn3YimmDtVC5AKj4CxKA39SHzJHxZkkY4";
    const ORPHAN_YULS_B: &str = "43vWz9GrY4mztfRNdFU4k3dudJDRTrjGPU9VoorWqMTs";
    /// `kk-0xjokker`'s two.
    const ORPHAN_JOKKER_A: &str = "4xQguonkrykFNxmzkNMieZ66ZQmr4ACU4HzqrHYEoHpy";
    const ORPHAN_JOKKER_B: &str = "RfkykvJAAR5Dfzwxzt76w34QxLpXxBrtzm89e9G7goK";

    /// The agent wallets those identities were meant to reach. KarmaKadabra's
    /// report truncates them (`Drc9BkJc...`, `BXPKpV6f...`), and a fabricated
    /// base58 pubkey would be worse than an honest placeholder, so the tests use
    /// fresh ones: nothing here depends on their value, only on their role.
    fn agent_yuls() -> Pubkey {
        Pubkey::new_unique()
    }
    fn agent_jokker() -> Pubkey {
        Pubkey::new_unique()
    }

    fn registry() -> RegistryContext {
        let collection = Pubkey::from_str(MAINNET_COLLECTION).unwrap();
        let programs = get_program_ids(&Network::Solana).unwrap();
        RegistryContext {
            root_config: crate::erc8004::solana::derive_root_config_pda(&programs.agent_registry).0,
            registry_config: crate::erc8004::solana::derive_registry_config_pda(
                &programs.agent_registry,
                &collection,
            )
            .0,
            collection,
            authority: Pubkey::new_unique(),
        }
    }

    fn agent_account(owner: &Pubkey, asset: &Pubkey, uri: &str) -> AgentAccount {
        AgentAccount {
            collection: Pubkey::from_str(MAINNET_COLLECTION).unwrap().to_bytes(),
            creator: owner.to_bytes(),
            owner: owner.to_bytes(),
            asset: asset.to_bytes(),
            bump: 254,
            atom_enabled: 0,
            agent_wallet: None,
            feedback_digest: [0u8; 32],
            feedback_count: 0,
            response_digest: [0u8; 32],
            response_count: 0,
            revoke_digest: [0u8; 32],
            revoke_count: 0,
            parent_asset: None,
            parent_locked: 0,
            col_locked: 0,
            agent_uri: uri.to_string(),
            nft_name: String::new(),
            col: String::new(),
        }
    }

    // ── (a) a complete mint is one transaction, and says so ──────────────────

    #[test]
    fn full_mint_fits_in_one_transaction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = Pubkey::new_unique();

        let plan = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: "https://karmakadabra.xyz/agents/kk-0xyuls.json",
            metadata: &[],
            recipient: Some(&recipient),
        });

        // Register, InitializeStats, TransferAgent -- in that order, because
        // only the owner may initialize and the transfer ends our ownership.
        assert_eq!(plan.instructions.len(), 3);
        assert_eq!(plan.instructions[0].program_id, programs.agent_registry);
        assert_eq!(plan.instructions[1].program_id, programs.atom_engine);
        assert_eq!(plan.instructions[2].program_id, programs.agent_registry);

        assert!(
            plan.is_atomic(),
            "the three-instruction mint must fit one transaction, got {} bytes / {:?}",
            plan.serialized_len,
            plan.not_atomic
        );
        assert!(
            plan.serialized_len <= PACKET_DATA_SIZE,
            "{} > {}",
            plan.serialized_len,
            PACKET_DATA_SIZE
        );
        // Three instructions are granted 600,000 CU by default: exactly the sum
        // of what the three get today running one per transaction.
        assert_eq!(plan.compute_units, 600_000);
        assert!(plan.creates_asset && plan.creates_stats && plan.transfers);
    }

    #[test]
    fn a_maximum_length_uri_still_fits_one_transaction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = Pubkey::new_unique();

        // MAX_URI_LEN in the registry program.
        let uri = "u".repeat(250);
        let plan = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: &uri,
            metadata: &[],
            recipient: Some(&recipient),
        });

        assert!(
            plan.is_atomic(),
            "the longest URI the program accepts must still ride in one transaction: {:?}",
            plan.not_atomic
        );
    }

    /// The measurement the spec asked for, pinned so a future change to the
    /// instructions cannot quietly push the mint past a limit.
    ///
    /// Measured 2026-09-09 on this code: a three-instruction mint serializes to
    /// 685 bytes with KarmaKadabra's URI and 890 with the longest URI the
    /// program accepts, against a 1232-byte packet. Compute is 600,000 units,
    /// which is exactly the sum of the three 200,000-unit budgets the same three
    /// instructions get today running one per transaction.
    #[test]
    fn the_bundle_measurement_is_pinned() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = Pubkey::new_unique();

        let short = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: "https://karmakadabra.xyz/agents/kk-0xyuls.json",
            metadata: &[],
            recipient: Some(&recipient),
        });
        assert_eq!(short.serialized_len, 685);

        let longest = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: &"u".repeat(250),
            metadata: &[],
            recipient: Some(&recipient),
        });
        assert_eq!(longest.serialized_len, 890);

        // Room to spare on both counts, which is why bundling is the default
        // rather than an optimisation to be attempted and abandoned.
        assert!(longest.serialized_len * 100 / PACKET_DATA_SIZE < 80);
        assert_eq!(longest.compute_units, 600_000);
        assert_eq!(MAX_BUNDLED_INSTRUCTIONS, 7);
    }

    #[test]
    fn a_bundle_that_would_dilute_the_compute_budget_is_split() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();

        // register + 6 metadata + stats + transfer = 9 instructions.
        let metadata: Vec<(String, Vec<u8>)> =
            (0..6).map(|i| (format!("k{i}"), vec![0u8; 8])).collect();

        let plan = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: "https://example.test/a.json",
            metadata: &metadata,
            recipient: Some(&recipient),
        });

        assert_eq!(plan.instructions.len(), 9);
        assert!(!plan.is_atomic());
        assert!(matches!(
            plan.not_atomic,
            Some(NotAtomic::ComputeDiluted {
                instructions: 9,
                limit: 7
            })
        ));
    }

    #[test]
    fn a_bundle_over_the_packet_limit_is_split() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::new_unique();

        // One metadata entry big enough to blow the packet on its own, while
        // staying inside the instruction count that keeps the compute budget.
        let metadata = vec![("big".to_string(), vec![0u8; 900])];

        let plan = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: "https://example.test/a.json",
            metadata: &metadata,
            recipient: None,
        });

        assert!(plan.instructions.len() <= MAX_BUNDLED_INSTRUCTIONS);
        assert!(matches!(
            plan.not_atomic,
            Some(NotAtomic::TooLarge {
                limit: PACKET_DATA_SIZE,
                ..
            })
        ));
    }

    #[test]
    fn every_step_has_a_code_and_the_order_is_the_one_the_program_requires() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();

        let plan = plan_mint(&MintRequest {
            programs: &programs,
            registry: &registry,
            asset: &asset,
            fee_payer: &fee_payer,
            agent_uri: "https://example.test/a.json",
            metadata: &[("website".to_string(), b"https://example.test".to_vec())],
            recipient: Some(&recipient),
        });

        // Metadata and stats both need the facilitator to still be the owner, so
        // both precede the transfer. Getting this order wrong delivers an
        // identity nobody can ever initialize or annotate.
        assert_eq!(
            plan.steps,
            vec![
                MintStep::Register,
                MintStep::Metadata,
                MintStep::InitializeStats,
                MintStep::Transfer,
            ]
        );
        assert_eq!(plan.steps.len(), plan.instructions.len());

        assert_eq!(error_code_for(MintStep::Register), ERROR_REGISTER_FAILED);
        assert_eq!(error_code_for(MintStep::Metadata), ERROR_METADATA_FAILED);
        assert_eq!(
            error_code_for(MintStep::InitializeStats),
            ERROR_STATS_FAILED
        );
        assert_eq!(error_code_for(MintStep::Transfer), ERROR_TRANSFER_FAILED);
    }

    #[test]
    fn an_atomic_mint_that_reverted_left_nothing_behind() {
        let outcome = MintOutcome {
            asset: Pubkey::new_unique(),
            fee_payer: Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap(),
            recipient: Some(Pubkey::new_unique()),
            registered: false,
            register_tx: None,
            stats_tx: None,
            transfer_tx: None,
            stats_ready: false,
            resumed: false,
            metadata_skipped: false,
            atomic: true,
            funds: None,
            failure: Some((
                ERROR_MINT_TRANSACTION_FAILED.to_string(),
                "RPC error: Transaction failed".to_string(),
            )),
        };

        // Nothing landed, so there is no stranded asset and no instruction to
        // resume: the caller can retry from scratch.
        assert_eq!(outcome.status(), MintStatus::NotMinted);
        assert_eq!(outcome.owner(), outcome.fee_payer);
        assert_eq!(
            outcome.error_message().as_deref(),
            Some("RPC error: Transaction failed")
        );
    }

    #[test]
    fn a_complete_outcome_reports_complete() {
        let asset = Pubkey::new_unique();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = Pubkey::new_unique();
        let sig = TransactionHash::Solana([7u8; 64]);

        let outcome = MintOutcome {
            asset,
            fee_payer,
            recipient: Some(recipient),
            registered: true,
            register_tx: Some(sig.clone()),
            stats_tx: Some(sig.clone()),
            transfer_tx: Some(sig.clone()),
            stats_ready: true,
            resumed: false,
            metadata_skipped: false,
            atomic: true,
            funds: None,
            failure: None,
        };

        assert_eq!(outcome.status(), MintStatus::Complete);
        assert_eq!(outcome.owner(), recipient);
        assert!(outcome.error_message().is_none());

        let report = outcome.report();
        assert!(report.atomic);
        assert!(!report.resumed);
        assert!(report.error_code.is_none());
        // An atomic mint reports one signature in all three slots because one
        // transaction carried all three instructions.
        assert_eq!(report.stats_transaction, Some(sig));
    }

    // ── (b) a mint whose transfer never lands must not read as success ────────

    #[test]
    fn a_mint_whose_transfer_failed_is_pending_not_successful() {
        let asset = Pubkey::from_str(ORPHAN_YULS_A).unwrap();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = agent_yuls();

        let outcome = MintOutcome {
            asset,
            fee_payer,
            recipient: Some(recipient),
            registered: true,
            register_tx: Some(TransactionHash::Solana([1u8; 64])),
            stats_tx: Some(TransactionHash::Solana([2u8; 64])),
            transfer_tx: None,
            stats_ready: true,
            resumed: false,
            metadata_skipped: false,
            atomic: false,
            funds: None,
            failure: Some((
                ERROR_TRANSFER_FAILED.to_string(),
                "RPC error: Transaction failed".to_string(),
            )),
        };

        assert_eq!(outcome.status(), MintStatus::PendingTransfer);
        assert!(!outcome.status().is_complete());
        // The facilitator, not the agent, is the owner. This is the fact that
        // made `GET /identity/solana/owner/<agent>` answer 404 while the mint
        // had answered success.
        assert_eq!(outcome.owner(), fee_payer);

        let report = outcome.report();
        assert_eq!(report.status, MintStatus::PendingTransfer);
        assert_eq!(report.error_code.as_deref(), Some(ERROR_TRANSFER_FAILED));
        // The signatures that did land travel with the pending state.
        assert!(report.stats_transaction.is_some());

        let message = outcome
            .error_message()
            .expect("a pending mint explains itself");
        assert!(message.contains("still held by the facilitator"));
        assert!(message.contains("resume this identity rather than mint another"));
    }

    #[test]
    fn a_mint_that_stopped_before_its_stats_is_pending_stats() {
        let outcome = MintOutcome {
            asset: Pubkey::from_str(ORPHAN_JOKKER_A).unwrap(),
            fee_payer: Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap(),
            recipient: Some(agent_jokker()),
            registered: true,
            register_tx: Some(TransactionHash::Solana([1u8; 64])),
            stats_tx: None,
            transfer_tx: None,
            stats_ready: false,
            resumed: false,
            metadata_skipped: false,
            atomic: false,
            funds: None,
            failure: Some((ERROR_STATS_FAILED.to_string(), "insufficient funds".into())),
        };

        assert_eq!(outcome.status(), MintStatus::PendingStats);
        // Crucially NOT transferred: after a transfer nobody can initialize the
        // stats any more, so the staged path must stop here rather than deliver
        // an identity whose feedback would never score.
        assert!(outcome.transfer_tx.is_none());
    }

    #[test]
    fn a_mint_with_no_recipient_is_complete_once_its_stats_exist() {
        let outcome = MintOutcome {
            asset: Pubkey::new_unique(),
            fee_payer: Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap(),
            recipient: None,
            registered: true,
            register_tx: Some(TransactionHash::Solana([1u8; 64])),
            stats_tx: Some(TransactionHash::Solana([1u8; 64])),
            transfer_tx: None,
            stats_ready: true,
            resumed: false,
            metadata_skipped: false,
            atomic: true,
            funds: None,
            failure: None,
        };

        // Nobody asked for a transfer, so facilitator ownership is the end state
        // that was requested -- not a half-finished one.
        assert_eq!(outcome.status(), MintStatus::Complete);
    }

    // ── (c) a retry resumes the stranded identity instead of minting another ──

    #[test]
    fn a_retry_resumes_the_stranded_identity() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let uri = "https://karmakadabra.xyz/agents/kk-0xyuls.json";

        let orphan = Pubkey::from_str(ORPHAN_YULS_A).unwrap();
        let held = vec![(
            derive_agent_pda(
                &orphan,
                &get_program_ids(&Network::Solana).unwrap().agent_registry,
            )
            .0,
            agent_account(&fee_payer, &orphan, uri),
        )];

        assert_eq!(
            decide_mint(uri, &fee_payer, &held),
            MintDecision::Resume { asset: orphan },
            "the second call must adopt the first call's asset"
        );
    }

    #[test]
    fn a_resume_plan_does_not_mint_a_second_asset() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let orphan = Pubkey::from_str(ORPHAN_JOKKER_B).unwrap();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = agent_jokker();

        let plan = plan_resume(
            &MintRequest {
                programs: &programs,
                registry: &registry,
                asset: &orphan,
                fee_payer: &fee_payer,
                agent_uri: "https://karmakadabra.xyz/agents/kk-0xjokker.json",
                metadata: &[],
                recipient: Some(&recipient),
            },
            true,
        );

        assert!(!plan.creates_asset, "a resume must never re-register");
        assert_eq!(plan.instructions.len(), 2);
        assert_eq!(plan.instructions[0].program_id, programs.atom_engine);
        assert_eq!(plan.instructions[1].program_id, programs.agent_registry);
        // The transfer names the asset the first attempt created, not a new one.
        assert!(plan.instructions[1]
            .accounts
            .iter()
            .any(|a| a.pubkey == orphan));
        assert!(plan.is_atomic());
    }

    #[test]
    fn a_resume_whose_stats_already_exist_only_transfers() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let registry = registry();
        let orphan = Pubkey::from_str(ORPHAN_YULS_B).unwrap();
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let recipient = agent_yuls();

        let plan = plan_resume(
            &MintRequest {
                programs: &programs,
                registry: &registry,
                asset: &orphan,
                fee_payer: &fee_payer,
                agent_uri: "https://karmakadabra.xyz/agents/kk-0xyuls.json",
                metadata: &[],
                recipient: Some(&recipient),
            },
            false,
        );

        assert_eq!(plan.instructions.len(), 1);
        assert!(!plan.creates_stats);
        // The one instruction left is the transfer: registry program, six
        // accounts, and the recipient in the new-owner slot.
        assert_eq!(plan.instructions[0].program_id, programs.agent_registry);
        assert_eq!(plan.instructions[0].accounts.len(), 6);
        assert_eq!(plan.instructions[0].accounts[4].pubkey, recipient);
    }

    #[test]
    fn a_different_uri_does_not_adopt_someone_elses_orphan() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let orphan = Pubkey::from_str(ORPHAN_YULS_A).unwrap();
        let held = vec![(
            Pubkey::new_unique(),
            agent_account(
                &fee_payer,
                &orphan,
                "https://karmakadabra.xyz/agents/kk-0xyuls.json",
            ),
        )];

        assert_eq!(
            decide_mint(
                "https://karmakadabra.xyz/agents/kk-0xjokker.json",
                &fee_payer,
                &held
            ),
            MintDecision::Fresh
        );
    }

    #[test]
    fn an_empty_uri_never_adopts_anything() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let held = vec![(
            Pubkey::new_unique(),
            agent_account(&fee_payer, &Pubkey::from_str(ORPHAN_YULS_A).unwrap(), ""),
        )];

        // `agent_uri` is #[serde(default)], so an absent one must not be an
        // identity key -- otherwise one URI-less call adopts an unrelated asset.
        assert_eq!(decide_mint("", &fee_payer, &held), MintDecision::Fresh);
    }

    #[test]
    fn an_asset_someone_else_owns_is_never_resumed() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let someone_else = Pubkey::new_unique();
        let uri = "https://karmakadabra.xyz/agents/kk-0xyuls.json";

        let held = vec![(
            Pubkey::new_unique(),
            agent_account(
                &someone_else,
                &Pubkey::from_str(ORPHAN_YULS_A).unwrap(),
                uri,
            ),
        )];

        assert_eq!(decide_mint(uri, &fee_payer, &held), MintDecision::Fresh);
    }

    #[test]
    fn all_four_stranded_assets_are_reachable_by_their_uri() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let yuls_uri = "https://karmakadabra.xyz/agents/kk-0xyuls.json";
        let jokker_uri = "https://karmakadabra.xyz/agents/kk-0xjokker.json";

        // Ordering matches `find_agents_by_owner`, which sorts by asset pubkey.
        let mut held: Vec<(Pubkey, AgentAccount)> = vec![
            (ORPHAN_YULS_A, yuls_uri),
            (ORPHAN_YULS_B, yuls_uri),
            (ORPHAN_JOKKER_A, jokker_uri),
            (ORPHAN_JOKKER_B, jokker_uri),
        ]
        .into_iter()
        .map(|(asset, uri)| {
            let asset = Pubkey::from_str(asset).unwrap();
            (Pubkey::new_unique(), agent_account(&fee_payer, &asset, uri))
        })
        .collect();
        held.sort_by_key(|(_, a)| a.asset);

        // Each retry adopts one of that agent's two orphans; a second retry
        // after the first is delivered picks up the other. Neither mints a new
        // asset, which is what stops the orphan count from growing.
        let yuls = decide_mint(yuls_uri, &fee_payer, &held);
        let jokker = decide_mint(jokker_uri, &fee_payer, &held);

        let yuls_assets = [
            Pubkey::from_str(ORPHAN_YULS_A).unwrap(),
            Pubkey::from_str(ORPHAN_YULS_B).unwrap(),
        ];
        let jokker_assets = [
            Pubkey::from_str(ORPHAN_JOKKER_A).unwrap(),
            Pubkey::from_str(ORPHAN_JOKKER_B).unwrap(),
        ];
        match yuls {
            MintDecision::Resume { asset } => assert!(yuls_assets.contains(&asset)),
            MintDecision::Fresh => panic!("kk-0xyuls's orphan was not found"),
        }
        match jokker {
            MintDecision::Resume { asset } => assert!(jokker_assets.contains(&asset)),
            MintDecision::Fresh => panic!("kk-0xjokker's orphan was not found"),
        }
    }

    // ── (d) an underfunded fee payer is a named error, not an RPC -32002 ──────

    #[test]
    fn an_underfunded_fee_payer_is_refused_before_the_chain() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let cost = estimate_mint_cost(true, true, &[], 1, 2);

        // The balance the fee payer actually held when 10 of KarmaKadabra's 20
        // mints failed with "-32002 Transaction simulation failed".
        let available = 866_000;
        let funds = FeePayerFunds::new(&fee_payer, available, &cost);

        assert!(!funds.is_sufficient());
        assert_eq!(funds.mints_remaining, 0);
        assert_eq!(funds.available_lamports, available);
        assert!(funds.required_lamports > available);

        let outcome = refusal(
            fee_payer,
            ERROR_FEE_PAYER_INSUFFICIENT_BALANCE,
            String::new(),
            Some(funds),
        );
        assert_eq!(outcome.status(), MintStatus::NotMinted);
        // Nothing was broadcast, so there is nothing to clean up afterwards.
        assert!(!outcome.registered);
        assert!(outcome.register_tx.is_none());
        assert_eq!(
            outcome.report().error_code.as_deref(),
            Some(ERROR_FEE_PAYER_INSUFFICIENT_BALANCE)
        );
        let reported = outcome
            .report()
            .fee_payer
            .expect("the numbers travel with the refusal");
        assert_eq!(reported.address, FACILITATOR_FEE_PAYER);
        assert_eq!(reported.available_lamports, available);
    }

    #[test]
    fn a_funded_fee_payer_passes_and_reports_its_headroom() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let cost = estimate_mint_cost(true, true, &[], 1, 2);

        // One SOL.
        let funds = FeePayerFunds::new(&fee_payer, LAMPORTS_PER_SOL, &cost);
        assert!(funds.is_sufficient());
        assert!(
            funds.mints_remaining >= 50,
            "one SOL should cover at least 50 mints, got {}",
            funds.mints_remaining
        );
    }

    #[test]
    fn the_cost_estimate_prices_rent_not_just_fees() {
        let bare = estimate_mint_cost(true, true, &[], 1, 2);
        // Rent dominates by three orders of magnitude; a mint priced on fees
        // alone would pass a preflight and then fail on chain.
        assert!(bare.rent_lamports > bare.fee_lamports * 100);
        assert_eq!(bare.fee_lamports, 2 * LAMPORTS_PER_SIGNATURE);

        // A resume pays neither the asset nor the agent PDA again.
        let resume = estimate_mint_cost(false, true, &[], 1, 1);
        assert!(resume.total_lamports() < bare.total_lamports());

        // A resume that only transfers costs a signature and nothing else.
        let transfer_only = estimate_mint_cost(false, false, &[], 1, 1);
        assert_eq!(transfer_only.rent_lamports, 0);
        assert_eq!(transfer_only.total_lamports(), LAMPORTS_PER_SIGNATURE);

        // Metadata entries each add their own rent.
        let with_metadata = estimate_mint_cost(true, true, &[("website".into(), 64)], 1, 2);
        assert!(with_metadata.rent_lamports > bare.rent_lamports);
    }

    #[test]
    fn the_staged_path_is_priced_for_every_signature_it_sends() {
        // Three transactions, one signature each except register's two.
        let staged = estimate_mint_cost(true, true, &[], 3, 4);
        let atomic = estimate_mint_cost(true, true, &[], 1, 2);

        assert_eq!(staged.rent_lamports, atomic.rent_lamports);
        assert!(staged.fee_lamports > atomic.fee_lamports);
        assert_eq!(staged.transactions, 3);
    }

    #[test]
    fn the_measured_mint_cost_is_in_the_range_karmakadabra_saw() {
        let cost = estimate_mint_cost(true, true, &[], 1, 2);
        // 0.0134 SOL: 6,096,960 for the agent PDA, 4,795,440 for the ATOM stats,
        // 2,500,000 for the Core asset, 10,000 in signature fees. The alarm in
        // `terraform/environments/production/alerts.tf` is sized on this number.
        assert_eq!(cost.total_lamports(), 13_402_400);
        // KK measured 0.0044-0.0089 SOL for the register transaction alone. The
        // whole three-instruction mint also pays the ATOM stats rent, so the
        // total lands above that band and below two hundredths of a SOL. If this
        // ever fails, the constants above drifted from the chain.
        assert!(
            cost.total_lamports() > 8_900_000,
            "{} lamports is below KK's measured register cost",
            cost.total_lamports()
        );
        assert!(
            cost.total_lamports() < 20_000_000,
            "{} lamports is implausibly high for one mint",
            cost.total_lamports()
        );
    }

    #[test]
    fn zero_balance_reports_zero_headroom_rather_than_dividing_by_it() {
        let cost = estimate_mint_cost(true, true, &[], 1, 2);
        assert_eq!(cost.mints_remaining(0), 0);

        // A plan that creates nothing and sends nothing has no price, and must
        // not panic when asked how many of it fit in a balance.
        let free = estimate_mint_cost(false, false, &[], 0, 0);
        assert_eq!(free.total_lamports(), 0);
        assert_eq!(free.mints_remaining(0), u64::MAX);
    }

    #[test]
    fn lamports_render_as_sol_without_losing_precision() {
        assert_eq!(lamports_to_sol_string(866_000), "0.000866000");
        assert_eq!(lamports_to_sol_string(LAMPORTS_PER_SOL), "1.000000000");
    }

    #[test]
    fn the_report_serializes_the_fields_a_client_branches_on() {
        let fee_payer = Pubkey::from_str(FACILITATOR_FEE_PAYER).unwrap();
        let cost = estimate_mint_cost(true, true, &[], 1, 2);
        let report = SolanaMintReport {
            status: MintStatus::PendingTransfer,
            error_code: Some(ERROR_TRANSFER_FAILED.to_string()),
            stats_transaction: Some(TransactionHash::Solana([3u8; 64])),
            resumed: true,
            atomic: false,
            metadata_skipped: false,
            fee_payer: Some(FeePayerFunds::new(&fee_payer, 866_000, &cost)),
        };

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["status"], "pending_transfer");
        assert_eq!(json["errorCode"], ERROR_TRANSFER_FAILED);
        assert_eq!(json["resumed"], true);
        assert_eq!(json["atomic"], false);
        assert_eq!(json["feePayer"]["address"], FACILITATOR_FEE_PAYER);
        assert_eq!(json["feePayer"]["availableLamports"], 866_000);
        assert!(json["feePayer"]["requiredLamports"].as_u64().unwrap() > 866_000);

        // A clean mint carries no error code at all rather than a null.
        let clean = SolanaMintReport {
            status: MintStatus::Complete,
            error_code: None,
            stats_transaction: None,
            resumed: false,
            atomic: true,
            metadata_skipped: false,
            fee_payer: None,
        };
        let json = serde_json::to_value(&clean).unwrap();
        assert_eq!(json["status"], "complete");
        assert!(json.get("errorCode").is_none());
        assert!(json.get("feePayer").is_none());
    }
}
