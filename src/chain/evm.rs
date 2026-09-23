//! x402 EVM flow: verification (off-chain) and settlement (on-chain).
//!
//! - **Verify**: simulate signature validity and transfer atomically in a single `eth_call`.
//!   For 6492 signatures, we call the universal validator which may *prepare* (deploy) the
//!   counterfactual wallet inside the same simulation.
//! - **Settle**: if the signer wallet is not yet deployed, we deploy it (via the 6492
//!   factory+calldata) and then call ERC-3009 `transferWithAuthorization` in a real tx.
//!
//! Assumptions:
//! - Target tokens implement ERC-3009 and support ERC-1271 for contract signers.
//! - The validator contract exists at [`VALIDATOR_ADDRESS`] on supported chains.
//!
//! Invariants:
//! - Settlement is atomic: deploy (if needed) + transfer happen in a single user flow.
//! - Verification does not persist state.

use alloy::contract::SolCallBuilder;
use alloy::dyn_abi::SolType;
use alloy::eips::BlockId;
use alloy::network::{
    Ethereum as AlloyEthereum, EthereumWallet, NetworkWallet, TransactionBuilder,
};
use alloy::primitives::{address, Address, Bytes, FixedBytes, U256};
use alloy::providers::bindings::IMulticall3;
use alloy::providers::fillers::NonceManager;
use alloy::providers::fillers::{
    BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller, WalletFiller,
};
use alloy::providers::ProviderBuilder;
use alloy::providers::{
    Identity, MulticallItem, Provider, RootProvider, WalletProvider, MULTICALL3_ADDRESS,
};
use alloy::rpc::client::RpcClient;
use alloy::rpc::types::{TransactionReceipt, TransactionRequest};
use alloy::sol_types::{eip712_domain, Eip712Domain, SolCall, SolStruct};
use alloy::{hex, sol};
use async_trait::async_trait;
use dashmap::DashMap;
use std::future::{Future, IntoFuture};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{instrument, Instrument};
use tracing_core::Level;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::erc8004::{Erc8004Extension, ProofOfPayment};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::{
    exact_payment_tokens, AUSDDeployment, EURCDeployment, Network,
    PYUSDDeployment, USDCDeployment, USDGDeployment, USDTDeployment,
};
use crate::timestamp::UnixTimestamp;
use crate::types::{
    EvmAddress, EvmSignature, ExactPaymentPayload, FacilitatorErrorReason, HexEncodedNonce,
    MixedAddress, PaymentPayload, PaymentRequirements, Scheme, SettleRequest, SettleResponse,
    SupportedPaymentKind, SupportedPaymentKindExtra, SupportedPaymentKindsResponse,
    SupportedTokenInfo, TokenAmount, TransactionHash, TransferWithAuthorization, VerifyRequest,
    VerifyResponse, X402Version,
};

sol!(
    #[allow(missing_docs)]
    #[allow(clippy::too_many_arguments)]
    #[derive(Debug)]
    #[sol(rpc)]
    USDC,
    "abi/USDC.json"
);

sol! {
    #[allow(missing_docs)]
    #[allow(clippy::too_many_arguments)]
    #[derive(Debug)]
    #[sol(rpc)]
    Validator6492,
    "abi/Validator6492.json"
}

/// Signature verifier for EIP-6492, EIP-1271, EOA, universally deployed on the supported EVM chains
/// If absent on a target chain, verification will fail; you should deploy the validator there.
const VALIDATOR_ADDRESS: alloy::primitives::Address =
    address!("0xdAcD51A54883eb67D95FAEb2BBfdC4a9a6BD2a3B");

/// Is [`VALIDATOR_ADDRESS`] deployed on this chain?
///
/// "If absent on a target chain, verification will fail" is true but describes
/// the WRONG failure. An `eth_call` to an address with no code returns empty
/// data, so the 6492 branch does not refuse the signature: the multicall's
/// decode fails and the caller gets `Invalid contract call`, a message that
/// blames the token contract for a facilitator-side gap and reads differently
/// depending on which of the two multicall shapes ran. Worse on `/settle`,
/// where the same absence makes the counterfactual path submit a factory call
/// that cannot have been validated.
///
/// So the question is asked BEFORE the call, and the answer is a verdict.
///
/// Measured on Arc testnet 2026-09-16 at block 62,335,077:
/// `eth_getCode(0xdAcD51A54883eb67D95FAEb2BBfdC4a9a6BD2a3B)` = **0 bytes**.
/// This is not permanent -- Arachnid's CREATE2 factory
/// (`0x4e59b44847b379578588920cA78FbF26c0B4956C`) IS deployed there, 69 bytes,
/// so the validator can be replayed to its usual address. Deploying it is a
/// separate, reviewable change; deleting this arm without that deployment is
/// not.
///
/// Only 6492 is gated. `StructuredSignature::EIP1271` covers plain EOA
/// signatures too and never touches this contract, so an ordinary EOA payment
/// -- the whole of the first launch on Arc -- is unaffected.
///
/// **The default arm is fail-OPEN, so adding a network is not a no-op here.**
/// A new chain falls into `_ => true` and is thereby asserted to carry the
/// validator, without anyone having looked. Run
/// `eth_getCode(VALIDATOR_ADDRESS)` against the new chain and give it an
/// explicit arm when the answer is empty; letting it default is a claim about
/// a contract nobody checked.
// `matches!` would say the same thing in one line and hide the default arm.
// The arm is the point: it is fail-open, and a reader adding a network has to
// see it.
#[allow(clippy::match_like_matches_macro)]
const fn has_eip6492_validator(network: Network) -> bool {
    match network {
        // eth_getCode -> 0 bytes on both Arc networks, measured 2026-09-16.
        Network::Arc | Network::ArcTestnet => false,
        _ => true,
    }
}

/// Refuse a counterfactual smart-wallet (EIP-6492) signature on a chain where
/// the validator that would check it is not deployed.
///
/// A verdict on the request, not a transport failure: same input, same answer,
/// no RPC involved.
///
/// # Why this reads the raw bytes, and why it is called from where it is
///
/// It used to take the parsed [`SignedMessage`] and run in `verify` and
/// `settle` just after `SignedMessage::extract`. **That made it unreachable.**
/// Both endpoints call [`assert_valid_payment`] first, and that enforces a
/// signature of exactly 65 bytes; an EIP-6492 envelope is an ABI tuple plus a
/// 32-byte magic suffix and is never 65 bytes, so it was refused as
/// `invalid_signature_length` long before this could speak. Two mutants that
/// deleted those two call sites survived the whole suite for the simplest
/// possible reason: the lines did nothing.
///
/// So it now takes the bytes and runs inside `assert_valid_payment`, one call
/// site instead of two, ahead of the length rule. The predicate is the same one
/// `TryFrom<Vec<u8>> for StructuredSignature` uses -- the trailing magic -- and
/// deliberately does not decode the envelope: a malformed 6492 body is the
/// normal path's business, not this one's.
fn assert_signature_scheme_supported(
    network: Network,
    payer: EvmAddress,
    signature: &EvmSignature,
) -> Result<(), FacilitatorLocalError> {
    let bytes = &signature.0;
    let is_eip6492 = bytes.len() >= 32 && bytes[bytes.len() - 32..] == EIP6492_MAGIC_SUFFIX;
    if is_eip6492 && !has_eip6492_validator(network) {
        return Err(FacilitatorLocalError::InvalidSignature(
            payer.into(),
            format!(
                // `concat!`, not a `\`-continued literal: a continuation whose
                // backslash is lost leaves its indentation INSIDE the string,
                // and the reader of a 400 sees a run of spaces mid-sentence.
                // That is what shipped here, twice, at eighteen spaces each.
                // `no_double_spaces_in_the_eip6492_refusal` pins it.
                // `network` is passed, not captured: implicit capture does not
                // reach through `concat!`.
                concat!(
                    "EIP-6492 signatures are not supported on {}: ",
                    "the universal signature validator is not deployed there. ",
                    "Use an EOA signature on this network."
                ),
                network
            ),
        ));
    }
    Ok(())
}

/// Why [`send_call_estimated`] did not hand back a pending transaction.
#[derive(Debug)]
pub enum EstimatedSendError {
    /// Gas estimation executed the call and it reverted. Nothing was sent and
    /// no nonce was reserved.
    Reverted(alloy::contract::Error),
    /// The broadcast itself failed.
    Send(alloy::contract::Error),
}

impl std::fmt::Display for EstimatedSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reverted(e) => write!(f, "gas estimation reverted, transaction not sent: {e}"),
            Self::Send(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EstimatedSendError {}

/// Send a contract call from the shared signer, estimating gas BEFORE a nonce
/// is reserved.
///
/// A bare `call.send()` lets alloy fill gas and nonce CONCURRENTLY
/// (`JoinFill::prepare` is a `try_join!`), and `NonceFiller::prepare` commits
/// the allocation as soon as it runs. A call that reverts on estimation then
/// still consumes a nonce it never broadcasts, and every later write from the
/// signer queues behind the gap: on Monad (2026-08-24) one reverting
/// `/feedback` froze nonces 379-381 for 151-283 s. `EvmProvider::settle` has
/// carried this guard since 2026-08-28; this is the same guard for the
/// ERC-8004 writers that share its `PendingNonceManager` (`post_feedback`,
/// `post_revoke_feedback`, `post_append_response`, `run_evm_registration`,
/// `transfer_agent_nft`).
///
/// Same rules as `settle`: estimate against `latest`, and only an execution
/// revert stops the send -- a transport failure falls through to the filler,
/// so a flaky RPC behaves exactly as before. Estimating goes through the same
/// `FillProvider`, whose `estimate_gas` runs only `prepare_call_sync` (the
/// wallet's `from`), never the nonce filler.
pub async fn send_call_estimated<P, D>(
    call: alloy::contract::CallBuilder<P, D, AlloyEthereum>,
    network: Network,
) -> Result<alloy::providers::PendingTransactionBuilder<AlloyEthereum>, EstimatedSendError>
where
    P: Provider<AlloyEthereum>,
    D: alloy::contract::CallDecoder,
{
    let call = call.block(BlockId::latest());
    match call.estimate_gas().await {
        Ok(gas) => {
            crate::erc8004::daily_cap::mark_sent();
            call.gas(gas).send().await.map_err(EstimatedSendError::Send)
        }
        Err(e) => {
            let msg = format!("{e:?}");
            if crate::handlers::is_execution_revert(&msg) {
                tracing::warn!(
                    %network,
                    error = %msg,
                    "Gas estimation reverted; call not sent, no nonce consumed"
                );
                return Err(EstimatedSendError::Reverted(e));
            }
            tracing::warn!(
                %network,
                error = %msg,
                "Gas estimation unavailable, falling back to filler"
            );
            crate::erc8004::daily_cap::mark_sent();
            call.send().await.map_err(EstimatedSendError::Send)
        }
    }
}

/// Combined filler type for gas, blob gas, nonce, and chain ID.
type InnerFiller = JoinFill<
    GasFiller,
    JoinFill<BlobGasFiller, JoinFill<NonceFiller<PendingNonceManager>, ChainIdFiller>>,
>;

/// The fully composed Ethereum provider type used in this project.
///
/// Combines multiple filler layers for gas, nonce, chain ID, blob gas, and wallet signing,
/// and wraps a [`RootProvider`] for actual JSON-RPC communication.
pub type InnerProvider = FillProvider<
    JoinFill<JoinFill<Identity, InnerFiller>, WalletFiller<EthereumWallet>>,
    RootProvider,
>;

/// Chain descriptor used by the EVM provider.
///
/// Wraps a `Network` enum and the concrete `chain_id` used for EIP-155 and EIP-712.
#[derive(Clone, Copy, Debug)]
pub struct EvmChain {
    /// x402 network name (Base, Avalanche, etc.).
    pub network: Network,
    /// Numeric chain id used in transactions and EIP-712 domains.
    pub chain_id: u64,
}

impl EvmChain {
    /// Construct a chain descriptor from a network and chain id.
    pub fn new(network: Network, chain_id: u64) -> Self {
        Self { network, chain_id }
    }

    /// Returns the x402 network.
    pub fn network(&self) -> Network {
        self.network
    }
}

impl TryFrom<Network> for EvmChain {
    type Error = FacilitatorLocalError;

    /// Map a `Network` to its canonical `chain_id`.
    ///
    /// # Errors
    /// Returns [`FacilitatorLocalError::UnsupportedNetwork`] for non-EVM networks (e.g. Solana).
    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::BaseSepolia => Ok(EvmChain::new(value, 84532)),
            Network::Base => Ok(EvmChain::new(value, 8453)),
            Network::XdcMainnet => Ok(EvmChain::new(value, 50)),
            Network::AvalancheFuji => Ok(EvmChain::new(value, 43113)),
            Network::Avalanche => Ok(EvmChain::new(value, 43114)),
            Network::Solana => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::SolanaDevnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::PolygonAmoy => Ok(EvmChain::new(value, 80002)),
            Network::Polygon => Ok(EvmChain::new(value, 137)),
            Network::Optimism => Ok(EvmChain::new(value, 10)),
            Network::OptimismSepolia => Ok(EvmChain::new(value, 11155420)),
            Network::Celo => Ok(EvmChain::new(value, 42220)),
            // 11142220, not Alfajores' 44787: the RPCs answered 0xaa044c on
            // 2026-09-23. Through 2.39.0 this said 44787 while the transaction
            // filler asks the RPC, so transactions went to the right chain while
            // the EIP-712 domain we build named the wrong one.
            Network::CeloSepolia => Ok(EvmChain::new(value, 11142220)),
            Network::HyperEvm => Ok(EvmChain::new(value, 999)),
            // 998, not 333: rpc.hyperliquid-testnet.xyz/evm answered 0x3e6 on
            // 2026-09-23. Same defect as celo-sepolia above, through 2.39.0.
            Network::HyperEvmTestnet => Ok(EvmChain::new(value, 998)),
            Network::Sei => Ok(EvmChain::new(value, 1329)),
            Network::SeiTestnet => Ok(EvmChain::new(value, 1328)),
            Network::Ethereum => Ok(EvmChain::new(value, 1)),
            Network::EthereumSepolia => Ok(EvmChain::new(value, 11155111)),
            Network::Arbitrum => Ok(EvmChain::new(value, 42161)),
            Network::ArbitrumSepolia => Ok(EvmChain::new(value, 421614)),
            Network::Unichain => Ok(EvmChain::new(value, 130)),
            Network::UnichainSepolia => Ok(EvmChain::new(value, 1301)),
            Network::Monad => Ok(EvmChain::new(value, 143)),
            Network::Bsc => Ok(EvmChain::new(value, 56)),
            Network::SkaleBase => Ok(EvmChain::new(value, 1187947933)),
            Network::SkaleBaseSepolia => Ok(EvmChain::new(value, 324705682)),
            Network::Scroll => Ok(EvmChain::new(value, 534352)),
            Network::Robinhood => Ok(EvmChain::new(value, 4663)),
            Network::RobinhoodTestnet => Ok(EvmChain::new(value, 46630)),
            // `eth_chainId` answered 0x4cef52 = 5042002 on 2026-09-16 at block
            // 62,335,077. The chain id is what the EIP-712 domain commits to,
            // so a wrong one here does not mis-route a payment -- it makes
            // every signature recover a different address.
            Network::Arc => Ok(EvmChain::new(value, 5042)),
            Network::ArcTestnet => Ok(EvmChain::new(value, 5042002)),
            Network::Near => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::NearTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "hedera")]
            Network::Hedera | Network::HederaTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Stellar => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::StellarTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "xrpl")]
            Network::Xrpl => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "xrpl")]
            Network::XrplTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Fogo => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::FogoTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "algorand")]
            Network::Algorand => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "algorand")]
            Network::AlgorandTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "sui")]
            Network::Sui => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            #[cfg(feature = "sui")]
            Network::SuiTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }
}

/// A fully specified ERC-3009 authorization payload for EVM settlement.
pub struct ExactEvmPayment {
    /// Target chain for settlement.
    #[allow(dead_code)] // Just in case.
    pub chain: EvmChain,
    /// Authorized sender (`from`) — EOA or smart wallet.
    pub from: EvmAddress,
    /// Authorized recipient (`to`).
    pub to: EvmAddress,
    /// Transfer amount (token units).
    pub value: TokenAmount,
    /// Not valid before this timestamp (inclusive).
    pub valid_after: UnixTimestamp,
    /// Not valid at/after this timestamp (exclusive).
    pub valid_before: UnixTimestamp,
    /// Unique 32-byte nonce (prevents replay).
    pub nonce: HexEncodedNonce,
    /// Raw signature bytes (EIP-1271 or EIP-6492-wrapped).
    pub signature: EvmSignature,
}

/// One gwei, in wei.
const GWEI: u128 = 1_000_000_000;

/// How many times the latest base fee a transaction's cap is allowed to reach.
///
/// Same multiplier alloy's default estimator uses. Deliberately unchanged: the
/// term that actually protects this rail is [`Eip1559Floor::min_max_fee`], and
/// widening the multiplier instead would inflate the txpool reservation on every
/// chain to buy a buffer that still does not survive the swings we measured.
const BASE_FEE_MULTIPLIER: u128 = 2;

/// Explicit EIP-1559 fee bounds for one network.
///
/// `maxFeePerGas` is a CAP, not a payment -- EIP-1559 charges
/// `baseFee + priority` and refunds the rest -- so a generous floor costs
/// nothing but txpool reservation headroom. That asymmetry is the whole reason
/// floors are the right lever here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Eip1559Floor {
    /// Never tip below this. Zero means the node's own priority estimate stands.
    pub min_priority: u128,
    /// Never cap below this. Zero means "no floor": price on the multiplier
    /// alone.
    pub min_max_fee: u128,
    /// Base fee to assume when the node answers `eth_feeHistory` without one.
    pub fallback_base_fee: u128,
}

/// Fee floors per network. Only networks whose fee behaviour has been MEASURED
/// get one; everything else prices on the multiplier alone.
///
/// # Why Polygon has the floor it has
///
/// Polygon's base fee sits at ~250 gwei in steady state and periodically
/// collapses to ~0 for hours before snapping back within the hour. Measured
/// three times in three days (2026-09-01 23:02Z, 2026-09-02 01:54Z, 2026-09-03
/// 17:53Z; each recovery took 40-80 minutes and the last one ran 0 -> 248 gwei).
///
/// On 2026-09-03 at 21:28:00Z the facilitator priced an escrow `release` in the
/// third of those troughs. The base fee was 1.072 gwei, alloy's estimator gave
/// `2 * 1.072 + 30.1 = 32.25` gwei, and forty minutes later the base fee was
/// 248 gwei. That transaction -- nonce 1157 -- could never be mined again, and
/// because nonces are strictly ordered it froze the signer: 399 correctly
/// priced transactions stacked up behind it, their pooled
/// `gasLimit * maxFeePerGas` reached 82.80 of the wallet's 82.86 POL, and every
/// new Polygon settle was refused by the node for six days.
///
/// No multiplier over the *latest* base fee survives that -- the trough lasts
/// hours, so a window maximum is no help either. An absolute floor above the
/// steady state is the only term that does. 1000 gwei is 4x the observed
/// steady state and reserves 0.36 POL per in-flight settle against an 82 POL
/// balance, so it buys the protection without crowding the pool.
pub(crate) const fn eip1559_fee_floor(network: Network) -> Eip1559Floor {
    match network {
        // Unchanged from the hand-rolled Ethereum branch this table replaced:
        // alloy's auto-estimation was producing 0.08 gwei caps on L1.
        Network::Ethereum | Network::EthereumSepolia => Eip1559Floor {
            min_priority: GWEI,
            min_max_fee: 5 * GWEI,
            fallback_base_fee: 2 * GWEI,
        },
        Network::Polygon | Network::PolygonAmoy => Eip1559Floor {
            min_priority: 30 * GWEI,
            min_max_fee: 1000 * GWEI,
            fallback_base_fee: 250 * GWEI,
        },
        // Arc's documented MINIMUM `maxFeePerGas` is 20 gwei, and the chain
        // sits exactly on it: base fee, `eth_gasPrice` and every sample of
        // `eth_feeHistory` all read 20 gwei (measured 2026-09-15 16:20Z and
        // again 2026-09-16 03:02Z at block 62,335,077).
        //
        // The generic arm below cannot price that. Its `min_max_fee` is 0, so
        // it contributes no floor at all, and its `fallback_base_fee` of 2 gwei
        // -- what a failed `eth_feeHistory` read falls back to -- yields
        // `2 * 2 + 0.001 = 4.001` gwei, a FIFTH of the minimum the chain will
        // accept. That transaction is refused, and a refused settle holds a
        // nonce that nothing else can replace.
        //
        // `min_priority` deliberately stays at the generic 1 mwei. Circle
        // permits a zero tip and `eth_maxPriorityFeePerGas` returns 0, so there
        // is nothing measured here to justify more; the 1 gwei tip that the
        // Ethereum arm carries drained the mainnet signer in four days when it
        // was copied to chains that had not earned it (see below). What Arc
        // needs is the CAP, not the tip.
        //
        // Cost of the floor: the guide's ~65,000 gas for an EIP-3009 transfer
        // at 20 gwei is 0.0013 USDC, which is also what gas costs on this chain
        // -- USDC is the native token.
        Network::Arc | Network::ArcTestnet => Eip1559Floor {
            min_priority: 1_000_000,
            min_max_fee: 20 * GWEI,
            fallback_base_fee: 20 * GWEI,
        },
        // A 1 mwei tip floor, not 1 gwei and not 0.
        //
        // From 2026-09-10 to 2026-09-14 this arm carried a 1 gwei `min_priority`,
        // copied from the Ethereum branch, and every chain here paid it: on Base
        // (base fee 0.005 gwei, node tip 0.001 gwei) a settle cost 0.0001038 ETH
        // at 1.005 gwei instead of ~0.0000006 ETH, and the 1.01 gwei cap is also
        // what a node reserves against the signer's balance. The mainnet signer
        // ran dry in four days and every Base settle was refused.
        //
        // Zero is not the answer either. geth and op-geth admit no tip below
        // 1 wei to the pool (`txpool.PriceLimit`) and mine none below 1 mwei
        // (`miner.GasPrice`), and a zero floor is what a failed
        // `eth_maxPriorityFeePerGas` read falls back to -- as it is what
        // hyperevm and arbitrum quote outright. That transaction is refused or
        // never mined, and a hung settle holds a nonce with nothing to replace
        // it. 1 mwei is what Base, Optimism and Unichain quote anyway.
        _ => Eip1559Floor {
            min_priority: 1_000_000,
            min_max_fee: 0,
            fallback_base_fee: 2 * GWEI,
        },
    }
}

/// What [`EvmProvider::quote_eip1559_fees`] measured and decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Eip1559Quote {
    /// The latest block's base fee, or the floor's fallback when absent.
    pub base_fee: u128,
    /// `maxPriorityFeePerGas`.
    pub priority: u128,
    /// `maxFeePerGas`: the cap a node checks the signer's balance against.
    pub max_fee: u128,
}

/// Turn a base fee and the node's priority estimate into the pair actually set
/// on the transaction.
///
/// Split out from the send path so the arithmetic can be tested against the
/// real numbers from the 2026-09-03 incident without an RPC.
pub(crate) fn compute_eip1559_fees(
    base_fee: u128,
    rpc_priority: u128,
    floor: Eip1559Floor,
) -> (u128, u128) {
    let priority = rpc_priority.max(floor.min_priority);
    let max_fee = base_fee
        .saturating_mul(BASE_FEE_MULTIPLIER)
        .saturating_add(priority)
        .max(floor.min_max_fee);
    // A type-2 transaction with maxFeePerGas < maxPriorityFeePerGas is invalid
    // and every node rejects it. Reachable whenever a node reports a priority
    // above `BASE_FEE_MULTIPLIER * baseFee + min_priority`, which is exactly
    // what a chain in a base-fee trough reports.
    (priority, max_fee.max(priority))
}

/// EVM implementation of the x402 facilitator.
///
/// Holds a composed Alloy ethereum provider [`InnerProvider`],
/// an `eip1559` toggle for gas pricing strategy, and the `EvmChain` context.
#[derive(Debug)]
pub struct EvmProvider {
    /// Composed Alloy provider with all fillers.
    inner: InnerProvider,
    /// Whether network supports EIP-1559 gas pricing.
    eip1559: bool,
    /// Chain descriptor (network + chain ID).
    chain: EvmChain,
    /// Available signer addresses for round-robin selection.
    signer_addresses: Arc<Vec<Address>>,
    /// Current position in round-robin signer rotation.
    signer_cursor: Arc<AtomicUsize>,
    /// Nonce manager for resetting nonces on transaction failures.
    nonce_manager: PendingNonceManager,
}

impl EvmProvider {
    /// Build an [`EvmProvider`] from a pre-composed Alloy ethereum provider [`InnerProvider`].
    pub async fn try_new(
        wallet: EthereumWallet,
        rpc_url: &str,
        eip1559: bool,
        network: Network,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let chain = EvmChain::try_from(network)?;
        let signer_addresses: Vec<Address> =
            NetworkWallet::<AlloyEthereum>::signer_addresses(&wallet).collect();
        if signer_addresses.is_empty() {
            return Err("wallet must contain at least one signer".into());
        }
        let signer_addresses = Arc::new(signer_addresses);
        let signer_cursor = Arc::new(AtomicUsize::new(0));
        // Retry rate-limited RPC calls instead of failing the settle outright.
        // Alloy's default policy recognises HTTP 429 plus the provider-specific
        // rate-limit codes (-32005 Infura, -32016 Alchemy, -32012/-32007
        // QuickNode) and honours any backoff hint in the response. Execution
        // Market's INC-2026-07-06 was 258 of these on a shared 50 req/s Base
        // budget, each of which killed a settle or a refund with no retry.
        //
        // `compute_units_per_second` is a CLIENT-SIDE throttle and defaults to
        // effectively off, so this change only adds retries. Operators can dial
        // it down per deployment via `RPC_MAX_CU_PER_SECOND` to stay under a
        // known provider budget (alloy bills ~20 CU per request by default, so
        // 1000 CU/s is roughly 50 requests/s).
        const MAX_RATE_LIMIT_RETRIES: u32 = 3;
        const INITIAL_BACKOFF_MS: u64 = 200;
        const DEFAULT_MAX_CU_PER_SECOND: u64 = 10_000_000;
        let max_cu_per_second = std::env::var("RPC_MAX_CU_PER_SECOND")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_MAX_CU_PER_SECOND);

        // `.connect(rpc_url)` (the previous call here) hands reqwest a bare
        // `Client::default()` under the hood, and reqwest's own defaults for
        // `timeout` / `connect_timeout` / `read_timeout` are all `None` -- alloy
        // adds nothing on top. An RPC that accepts the TCP handshake and then
        // never answers held a settle open until the ALB's OWN 600s idle
        // timeout, not ours (CAUSA RAIZ #3,
        // docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md).
        // Timeouts come from `crate::chain` (shared with algorand/stellar/xrpl's
        // own `reqwest::Client`s, coordinated over IRC 2026-08-28) rather than a
        // second definition here -- "CONFIGURACION CENTRALIZADA" in CLAUDE.md.
        let http_client = reqwest::Client::builder()
            .user_agent("uvd-x402-facilitator")
            .timeout(crate::chain::rpc_http_timeout())
            .connect_timeout(crate::chain::rpc_http_connect_timeout())
            .build()
            .map_err(|e| format!("Failed to build HTTP client for {network}: {e}"))?;
        let parsed_rpc_url: url::Url = rpc_url
            .parse()
            .map_err(|e| format!("Invalid RPC URL for {network}: {e}"))?;

        // NOTE: `.http_with_client(..)` is SYNCHRONOUS -- it returns `RpcClient`
        // directly, not a `TransportResult`. No `.await`, no `.map_err` here;
        // both belonged to the `.connect(rpc_url).await` call this replaced.
        let client = RpcClient::builder()
            .layer(alloy::transports::layers::RetryBackoffLayer::new(
                MAX_RATE_LIMIT_RETRIES,
                INITIAL_BACKOFF_MS,
                max_cu_per_second,
            ))
            .http_with_client(http_client, parsed_rpc_url);

        // Create nonce manager explicitly so we can store a reference for error handling
        let nonce_manager = PendingNonceManager::default();

        // Build the filler stack: Gas -> BlobGas -> Nonce -> ChainId
        // This mirrors the InnerFiller type but with our custom nonce manager
        let filler = JoinFill::new(
            GasFiller,
            JoinFill::new(
                BlobGasFiller::default(),
                JoinFill::new(
                    NonceFiller::new(nonce_manager.clone()),
                    ChainIdFiller::default(),
                ),
            ),
        );

        let inner = ProviderBuilder::default()
            .filler(filler)
            .wallet(wallet)
            .connect_client(client);

        tracing::info!(network=%network, rpc=%crate::redact::rpc_url(rpc_url), signers=?signer_addresses, "Initialized provider");

        Ok(Self {
            inner,
            eip1559,
            chain,
            signer_addresses,
            signer_cursor,
            nonce_manager,
        })
    }

    /// Round-robin selection of next signer from wallet.
    fn next_signer_address(&self) -> Address {
        debug_assert!(!self.signer_addresses.is_empty());
        if self.signer_addresses.len() == 1 {
            self.signer_addresses[0]
        } else {
            let next =
                self.signer_cursor.fetch_add(1, Ordering::Relaxed) % self.signer_addresses.len();
            self.signer_addresses[next]
        }
    }
}

/// Trait for sending meta-transactions with custom target and calldata.
pub trait MetaEvmProvider {
    /// Error type for operations.
    type Error;
    /// Underlying provider type.
    type Inner: Provider;

    /// Returns reference to underlying provider.
    fn inner(&self) -> &Self::Inner;
    /// Returns reference to chain descriptor.
    fn chain(&self) -> &EvmChain;

    /// Whether the network supports EIP-1559 gas pricing.
    /// Legacy chains (e.g., SKALE) return false and need explicit gasPrice.
    fn is_eip1559(&self) -> bool;

    /// Sends a meta-transaction to the network.
    fn send_transaction(
        &self,
        tx: MetaTransaction,
    ) -> impl Future<Output = Result<TransactionReceipt, Self::Error>> + Send;
}

/// Meta-transaction parameters: target address, calldata, and required confirmations.
pub struct MetaTransaction {
    /// Target contract address.
    pub to: Address,
    /// Transaction calldata (encoded function call).
    pub calldata: Bytes,
    /// Number of block confirmations to wait for.
    pub confirmations: u64,
    /// EIP-7702 authorizations to install with this transaction, which makes it
    /// a type-4.
    ///
    /// Carried here rather than sent through a separate path on purpose: the
    /// relayed-feedback flow needs the same writer lease, the same nonce lane
    /// and the same receipt-status check as every other write. A second send
    /// path would be a second place to forget them.
    pub authorization_list: Option<Vec<alloy::eips::eip7702::SignedAuthorization>>,
}

impl MetaEvmProvider for EvmProvider {
    type Error = FacilitatorLocalError;
    type Inner = InnerProvider;

    fn inner(&self) -> &Self::Inner {
        &self.inner
    }

    fn chain(&self) -> &EvmChain {
        &self.chain
    }

    fn is_eip1559(&self) -> bool {
        self.eip1559
    }

    /// Send a meta-transaction with provided `to`, `calldata`, and automatically selected signer.
    ///
    /// This method constructs a transaction from the provided [`MetaTransaction`], automatically
    /// selects the next available signer using round-robin selection, and handles gas pricing
    /// based on whether the network supports EIP-1559.
    ///
    /// If the transaction fails at any point (during submission or receipt fetching), the nonce
    /// for the sending address is reset to force a fresh query on the next transaction. This
    /// ensures correctness even when transactions partially succeed (e.g., submitted but receipt
    /// fetch times out).
    ///
    /// # Gas Pricing Strategy
    ///
    /// - **EIP-1559 networks**: Uses automatic gas pricing via the provider's fillers.
    /// - **Legacy networks**: Fetches the current gas price using `get_gas_price()` and sets it explicitly.
    ///
    /// # Timeout Configuration
    ///
    /// Receipt fetching is subject to a configurable timeout:
    /// - Default: 30 seconds
    /// - Override via `TX_RECEIPT_TIMEOUT_SECS` environment variable
    /// - If the timeout expires, the nonce is reset and an error is returned
    ///
    /// # Parameters
    ///
    /// - `tx`: A [`MetaTransaction`] containing the target address and calldata.
    ///
    /// # Returns
    ///
    /// A [`TransactionReceipt`] once the transaction has been mined and confirmed.
    ///
    /// # Errors
    ///
    /// Returns [`FacilitatorLocalError::ContractCall`] if:
    /// - Gas price fetching fails (on legacy networks)
    /// - Transaction sending fails
    /// - Receipt retrieval fails or times out
    async fn send_transaction(
        &self,
        tx: MetaTransaction,
    ) -> Result<TransactionReceipt, Self::Error> {
        self.send_transaction_from(self.next_signer_address(), tx)
            .await
    }
}

impl EvmProvider {
    /// The single signer that identity-bound writes must always use.
    ///
    /// Some destinations bind the facilitator's address on-chain and cannot be
    /// rotated: the x402r PaymentOperator gates `release` and `refundInEscrow`
    /// behind a `StaticAddressCondition` pinning this exact EOA (verified on
    /// all 8 legacy mainnets and on SKALE — a call from any other sender
    /// reverts `ConditionNotMet`), and ERC-8004 reputation records are keyed by
    /// their author, so a rotated writer would fragment the facilitator's
    /// on-chain identity. `authorize` and plain EIP-3009 settles carry no such
    /// binding and stay on the round-robin path.
    pub fn pinned_signer(&self) -> Address {
        self.inner.default_signer_address()
    }

    /// Whether `address` is one of the signers this provider can sign with.
    ///
    /// Used to reject payloads that bind settlement to an address we do not
    /// hold, before spending gas on a transaction that can only revert.
    pub fn controls_signer(&self, address: Address) -> bool {
        self.signer_addresses.contains(&address)
    }

    /// Every signer this provider rotates through.
    ///
    /// For the readiness probe, which has to grade each one: a round-robin
    /// settle lands on any of them, so the emptiest decides.
    pub fn signer_addresses(&self) -> &[Address] {
        &self.signer_addresses
    }

    /// The EIP-1559 fee pair the send path sets right now, from the node's
    /// latest base fee and priority estimate and this network's floor.
    ///
    /// One implementation for both callers: the settle path and the readiness
    /// probe. A probe that priced differently from the transaction it predicts
    /// would report a margin the node does not grant.
    pub(crate) async fn quote_eip1559_fees(
        &self,
    ) -> Result<Eip1559Quote, alloy::transports::TransportError> {
        let floor = eip1559_fee_floor(self.chain.network);
        let fee_history = self
            .inner
            .get_fee_history(1, alloy::eips::BlockNumberOrTag::Latest, &[])
            .await?;
        let base_fee = fee_history
            .latest_block_base_fee()
            .unwrap_or(floor.fallback_base_fee);
        let rpc_priority = self
            .inner
            .get_max_priority_fee_per_gas()
            .await
            .unwrap_or(floor.min_priority);
        let (priority, max_fee) = compute_eip1559_fees(base_fee, rpc_priority, floor);
        Ok(Eip1559Quote {
            base_fee,
            priority,
            max_fee,
        })
    }

    /// The most per unit of gas the node will reserve against this signer's
    /// balance for the next transaction: `maxFeePerGas` on EIP-1559 chains,
    /// `gasPrice` on legacy ones.
    ///
    /// That, not the price finally paid, is what `eth_estimateGas` and the
    /// txpool check a balance against -- which is why a signer with enough to
    /// pay can still be refused.
    pub(crate) async fn quote_fee_cap(&self) -> Result<u128, alloy::transports::TransportError> {
        if self.eip1559 {
            Ok(self.quote_eip1559_fees().await?.max_fee)
        } else {
            self.inner.get_gas_price().await
        }
    }

    /// Sends a meta-transaction from a SPECIFIC signer instead of the
    /// round-robin one.
    ///
    /// Required wherever the destination contract binds `msg.sender`: the UPTO
    /// proxy reverts `UnauthorizedFacilitator` unless the sender equals the
    /// `facilitator` the payer signed into the Permit2 witness, and ERC-8004
    /// reputation records are keyed by author, so rotating them would split the
    /// facilitator's on-chain identity.
    pub async fn send_transaction_from(
        &self,
        from_address: Address,
        tx: MetaTransaction,
    ) -> Result<TransactionReceipt, FacilitatorLocalError> {
        // Hard error, never a fallback to the default or the round-robin
        // cursor: silently substituting a signer is exactly the failure this
        // API exists to prevent, and on a caller-pinned contract it produces a
        // revert that looks like a contract bug.
        if !self.controls_signer(from_address) {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "refusing to send from {from_address}: not a signer this facilitator holds"
            )));
        }

        // Only the elected writer may allocate nonces for the shared EOA. Two
        // ECS tasks overlap on every rolling deploy, and each keeps a private
        // nonce cache; without this gate they race for the same nonce and one
        // of them loses.
        //
        // This check is only the cheap one: it saves a gas estimate and a fee
        // lookup on a task that plainly cannot sign. The check that carries the
        // guarantee is the per-attempt `signing_permit` below -- between this
        // line and the broadcast sit an `eth_call`, a gas estimate and a nonce
        // resync, seconds in which a grant can end.
        if !crate::writer_lease::is_writer() {
            return Err(FacilitatorLocalError::WriterLeaseUnavailable(
                "this task holds no EVM writer grant".to_string(),
            ));
        }

        let to = tx.to;
        let calldata = tx.calldata;
        let confirmations = tx.confirmations;
        let authorization_list = tx.authorization_list;

        // Two retries rather than one: with several settles queued on the same
        // signer, the first retry can lose the race again to a sibling that
        // resynced a moment earlier. Each retry re-sends the same EIP-3009
        // authorization, which is safe — if the original mined, the guard below
        // catches it, and failing that the token rejects the replay.
        const MAX_NONCE_RETRIES: u32 = 2;

        // Snapshot confirmed TX count before sending. If this advances after a
        // nonce error, the "failed" TX was actually mined (RPC load-balancer
        // returned a stale error). Retrying would replay an already-consumed
        // EIP-3009 auth → "FiatTokenV2: invalid signature".
        //
        // `None` means the RPC never answered. It must NOT collapse to 0: a
        // rate-limited probe reading as nonce 0 makes the "already mined" guard
        // below trivially true for any funded signer, which silently suppresses
        // the one retry we allow — precisely under the rate-limit conditions
        // where the retry matters most.
        let pre_send_nonce: Option<u64> = self
            .inner
            .get_transaction_count(from_address)
            .await
            .inspect_err(
                |e| tracing::warn!(%from_address, error = ?e, "pre-send nonce probe failed"),
            )
            .ok();

        for attempt in 0..=MAX_NONCE_RETRIES {
            // Build TX request (calldata.clone() is O(1) - Bytes is ref-counted)
            let mut txr = TransactionRequest::default()
                .with_to(to)
                .with_from(from_address)
                .with_input(calldata.clone());

            // Present only for relayed ERC-8004 feedback, and what turns this
            // into a type-4 transaction: the rater's authorization delegating
            // their EOA to the FeedbackDelegate, so the registry observes THEM
            // as msg.sender while we pay.
            if let Some(list) = authorization_list.as_ref() {
                txr.authorization_list = Some(list.clone());
            }

            if !self.eip1559 {
                let gas: u128 = self
                    .inner
                    .get_gas_price()
                    .instrument(tracing::info_span!("get_gas_price"))
                    .await
                    .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                txr.set_gas_price(gas);
            } else {
                // Price EVERY EIP-1559 chain explicitly, not just Ethereum.
                //
                // Left to alloy's default estimator, a transaction's entire
                // buffer is `2 * baseFee`, measured on the latest block. That is
                // fine on a chain whose fee drifts and fatal on one whose fee
                // moves in steps: on 2026-09-03 Polygon went from 1.07 gwei to
                // 248 gwei in forty minutes, and the settle priced at the bottom
                // of that move froze the mainnet signer for six days. See
                // [`eip1559_fee_floor`] for the full account.
                let floor = eip1559_fee_floor(self.chain.network);
                match self.quote_eip1559_fees().await {
                    Ok(Eip1559Quote {
                        base_fee,
                        priority,
                        max_fee,
                    }) => {
                        txr.set_max_priority_fee_per_gas(priority);
                        txr.set_max_fee_per_gas(max_fee);
                        tracing::debug!(
                            network = %self.chain.network,
                            base_fee_gwei = base_fee / GWEI,
                            priority_gwei = priority / GWEI,
                            max_fee_gwei = max_fee / GWEI,
                            "EIP-1559 gas pricing"
                        );
                    }
                    Err(error) => {
                        // A chain with a measured floor gets it even here: that
                        // floor is the whole protection and dropping it on a
                        // flaky read reintroduces the failure. A chain without
                        // one is left untouched so alloy's filler still runs,
                        // which is exactly today's behaviour for it.
                        if floor.min_max_fee > 0 {
                            txr.set_max_priority_fee_per_gas(floor.min_priority);
                            txr.set_max_fee_per_gas(floor.min_max_fee.max(floor.min_priority));
                        }
                        tracing::warn!(
                            network = %self.chain.network,
                            ?error,
                            floored = floor.min_max_fee > 0,
                            "fee history unavailable; falling back for gas pricing"
                        );
                    }
                }
            }

            // Estimate gas BEFORE reserving a nonce.
            //
            // Alloy fills gas and nonce CONCURRENTLY — `JoinFill::prepare` is a
            // `try_join!` — and `NonceFiller::prepare` commits our allocation as
            // soon as it runs. So when estimation reverts, the nonce has already
            // been consumed for a transaction that is never broadcast, leaving a
            // gap that stalls every later settle from this signer. `/settle` is
            // unauthenticated, so a stream of reverting payloads was a self-DoS
            // on real settlements. Estimating first means a revert costs us
            // nothing, and the explicit limit also saves the filler's own
            // estimate round-trip on the happy path.
            //
            // The 5 ERC-8004 writers in `src/handlers.rs` share this
            // `PendingNonceManager`; they get the same guard through
            // `send_call_estimated` (top of this file), which is where the
            // nonce-reservation regression test lives.
            // Only a revert is treated as fatal here: if estimation fails for
            // transport reasons we fall through and let the filler try, which
            // preserves the previous behaviour on flaky RPCs.
            // Estimate against `latest`, not alloy's default `pending`: some
            // RPCs (publicnode's Fuji endpoint, 2026-08-11) reject state
            // execution on the pending block with `-32000 state not available`,
            // which is a transport error, not a revert — so it fell through to
            // the filler, whose own estimate defaults to `pending` again and
            // died identically. With the limit set here the filler never
            // re-estimates. `verify` already simulates via `.call()`, which
            // defaults to `latest` — this aligns settle with it.
            match self
                .inner
                .estimate_gas(txr.clone())
                .block(BlockId::latest())
                .await
            {
                Ok(gas) => {
                    // Head-room for state drift between estimate and inclusion.
                    txr.set_gas_limit(gas.saturating_mul(5) / 4);
                }
                Err(e) => {
                    let msg = format!("{e:?}");
                    if crate::handlers::is_execution_revert(&msg) {
                        tracing::warn!(
                            %from_address,
                            network = %self.chain.network,
                            error = %msg,
                            "Gas estimation reverted; no nonce consumed"
                        );
                        return Err(FacilitatorLocalError::ContractCall(msg));
                    }
                    tracing::warn!(
                        %from_address,
                        error = %msg,
                        "Gas estimation unavailable, falling back to filler"
                    );
                }
            }

            // The exclusive section starts here and ends when the broadcast
            // resolves. Everything above is read-only against the chain and
            // safe to run on any task; from here to `send_transaction` we are
            // allocating a nonce for a signer shared with every other task, and
            // that is the one thing exactly one process may do at a time.
            //
            // Refused rather than risked when the grant is nearly out: a
            // signature nobody can prove we were entitled to make is worse than
            // a 503 the caller can retry against the task that IS entitled.
            let permit = match crate::writer_lease::signing_permit() {
                Some(permit) => permit,
                None => {
                    tracing::warn!(
                        %from_address,
                        network = %self.chain.network,
                        attempt = attempt + 1,
                        "refusing to allocate a nonce: no writer grant with enough headroom"
                    );
                    return Err(FacilitatorLocalError::WriterLeaseUnavailable(
                        "no writer grant with enough headroom to broadcast".to_string(),
                    ));
                }
            };

            // Reserve the nonce explicitly so that a failure below can hand it
            // back precisely, rather than leaving the shared counter ahead of
            // the chain. `NonceFiller` short-circuits once the nonce is set.
            let reserved_nonce = match self
                .nonce_manager
                .get_next_nonce(&self.inner, from_address)
                .await
            {
                Ok(nonce) => {
                    txr.set_nonce(nonce);
                    Some(nonce)
                }
                Err(e) => {
                    // Could not reach the chain to resync; let the filler retry
                    // the allocation as part of the send.
                    tracing::warn!(%from_address, error = ?e, "Nonce reservation failed, deferring to filler");
                    None
                }
            };

            // Send transaction. An ERC-8004 write that got this far keeps its
            // place in the daily count (`erc8004::daily_cap`); the reverting
            // estimate above returned before it.
            crate::erc8004::daily_cap::mark_sent();
            // The hash of the bytes handed to the node, set the moment they are.
            // A send that fails after that may still have queued them, and then
            // the caller gets this hash to look up instead of an invitation to
            // retry.
            let mut broadcast: Option<alloy::primitives::TxHash> = None;
            let send_outcome = if crate::receipts::active() {
                use alloy::eips::Encodable2718;
                let filled = self.inner.fill(txr).await.map_err(|_| FacilitatorLocalError::ContractCall("receipt transaction preparation failed".into()))?;
                let envelope = filled.as_envelope().ok_or_else(|| FacilitatorLocalError::ContractCall("receipt transaction was not signed".into()))?;
                let signed = envelope.encoded_2718();
                let hash = alloy::primitives::keccak256(&signed);
                crate::receipts::prepared_evm(hash.to_string(), signed.clone())
                    .await
                    .map_err(FacilitatorLocalError::ContractCall)?;
                // Latched BEFORE the send: from here no failure can release the
                // receipt admission, whatever the node answers.
                crate::receipts::sending();
                broadcast = Some(hash);
                self.inner.send_raw_transaction(&signed).await
            } else {
                use alloy::eips::Encodable2718;
                crate::receipts::sending();
                // Filled and signed here, then sent raw: what `send_transaction`
                // does internally, split so that the hash exists before the
                // bytes leave and a failed fill stays apart from a failed send.
                // A fill that fails sent nothing and keeps the handling below; a
                // send that fails may have queued the transaction.
                match self.inner.fill(txr).await {
                    Ok(filled) => match filled.as_envelope() {
                        Some(envelope) => {
                            let signed = envelope.encoded_2718();
                            broadcast = Some(alloy::primitives::keccak256(&signed));
                            self.inner.send_raw_transaction(&signed).await
                        }
                        None => {
                            return Err(FacilitatorLocalError::ContractCall(
                                "transaction was not signed".into(),
                            ))
                        }
                    },
                    Err(e) => Err(e),
                }
            };

            // The permit is released as soon as the broadcast resolves, NOT
            // after the receipt: the receipt wait is up to 900s on Ethereum and
            // allocates nothing, so holding it across would make every handover
            // wait for a confirmation it has no stake in.
            let inside_tenancy = permit.still_valid();
            drop(permit);
            if !inside_tenancy {
                // The grant ended, or the lease changed hands, while this
                // broadcast was in flight. It is out either way -- refusing now
                // would only throw away the hash. Say so loudly: this is the
                // one observable that tells an operator the margins are too
                // tight for this network's RPC.
                tracing::error!(
                    %from_address,
                    network = %self.chain.network,
                    generation = crate::writer_lease::generation(),
                    "broadcast finished OUTSIDE the writer grant that authorised it; \
                     the handover margin is too small for this RPC's latency"
                );
            }

            match send_outcome {
                Ok(pending_tx) => {
                    // Log TX hash for debugging (visible on block explorers)
                    let tx_hash = *pending_tx.tx_hash();
                    tracing::info!(
                        %tx_hash,
                        %from_address,
                        network = %self.chain.network,
                        "Transaction submitted to mempool"
                    );

                    // TX submitted - wait for receipt with timeout
                    // Ethereum L1 uses 900s - L1 can take 10+ min during congestion
                    // (601s observed during Golden Flow testing 2026-02-21)
                    // Base mainnet requires longer timeout (90s) due to network congestion
                    // Other EVM chains use default 30s timeout
                    let default_timeout = match self.chain.network {
                        Network::Ethereum => 900,
                        Network::Base => 90,
                        _ => 30,
                    };
                    let timeout = std::time::Duration::from_secs(
                        std::env::var("TX_RECEIPT_TIMEOUT_SECS")
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(default_timeout),
                    );

                    let watcher = pending_tx
                        .with_required_confirmations(confirmations)
                        .with_timeout(Some(timeout));

                    return match watcher.get_receipt().await {
                        // A mined transaction is not a successful one. Without
                        // this check a reverted release/refundInEscrow came back
                        // as a receipt with a real hash and was reported to the
                        // merchant as `success: true` — the same defect class
                        // fixed for ERC-8004 in v1.49.0, still open on every
                        // other write path until now.
                        Ok(receipt) if !receipt.status() => {
                            tracing::error!(
                                tx_hash = %receipt.transaction_hash,
                                %from_address,
                                network = %self.chain.network,
                                "Transaction mined but REVERTED"
                            );
                            Err(FacilitatorLocalError::ContractCall(format!(
                                "transaction {} reverted on {}",
                                receipt.transaction_hash, self.chain.network
                            )))
                        }
                        Ok(receipt) => Ok(receipt),
                        Err(e) => {
                            // Receipt fetch failed (timeout or other) - reset nonce
                            // Do NOT retry: TX may have been mined, retrying could double-spend
                            self.nonce_manager.reset_nonce(from_address).await;
                            // ...and for the same reason the hash cannot be
                            // dropped here. `ContractCall` used to swallow it,
                            // so the caller got a correlation id only we can
                            // resolve for a transaction that may be sitting
                            // mined on chain -- with nothing to look up, the
                            // only move left is the retry this branch exists
                            // to prevent.
                            tracing::error!(
                                %tx_hash,
                                %from_address,
                                network = %self.chain.network,
                                error = ?e,
                                "Receipt never arrived; transaction may be mined"
                            );
                            Err(FacilitatorLocalError::SettlementUnconfirmed(
                                TransactionHash::Evm(tx_hash.0),
                                self.chain.network,
                            ))
                        }
                    };
                }
                Err(e) => {
                    let error_str = format!("{e:?}");

                    // A transaction the node refused before it ever entered the
                    // mempool consumed no nonce on-chain, so hand ours back
                    // instead of leaving a gap the whole signer queues behind.
                    // Anything ambiguous keeps the conservative reset.
                    match (reserved_nonce, is_pre_broadcast_rejection(&error_str)) {
                        (Some(nonce), true) => {
                            self.nonce_manager.release_nonce(from_address, nonce).await
                        }
                        // `nonce too high` is the one failure that tells us the
                        // chain is BEHIND our counter, so the high-water mark is
                        // what is wrong and must go. Anything else keeps the
                        // conservative reset that preserves it.
                        _ if is_nonce_too_high(&error_str) => {
                            self.nonce_manager.resync_to_chain(from_address).await
                        }
                        _ => self.nonce_manager.reset_nonce(from_address).await,
                    }

                    // The bytes reached the transport and the node never said it
                    // refused them: a timeout, a dropped connection, a gateway
                    // error, an answer that would not parse, or the node saying
                    // it already holds them. Any of those can end mined, so this
                    // is not retried here and not advertised as retryable to the
                    // caller, who gets the hash to look up instead.
                    if let Some(hash) = broadcast.filter(|_| broadcast_may_have_queued(&e)) {
                        tracing::error!(
                            tx_hash = %hash,
                            %from_address,
                            network = %self.chain.network,
                            error = %crate::redact::scrub_urls(&error_str),
                            "Broadcast outcome unknown; the transaction may be in a mempool"
                        );
                        return Err(FacilitatorLocalError::SettlementUnconfirmed(
                            TransactionHash::Evm(hash.0),
                            self.chain.network,
                        ));
                    }

                    if is_nonce_error(&error_str) && attempt < MAX_NONCE_RETRIES {
                        // Safety check: if the confirmed TX count advanced, the
                        // "failed" TX was actually mined by a different RPC node.
                        // Retrying would replay the same EIP-3009 auth and revert.
                        let post_nonce = self
                            .inner
                            .get_transaction_count(from_address)
                            .await
                            .inspect_err(|e| {
                                tracing::warn!(%from_address, error = ?e, "post-send nonce probe failed")
                            })
                            .ok();

                        // Only trust the guard when BOTH probes answered. If
                        // either failed we cannot tell a mined TX from a lost
                        // one, so we decline to retry: a replayed EIP-3009 auth
                        // burns gas and returns a misleading "invalid
                        // signature", and the caller can retry safely under its
                        // own idempotency key.
                        match (pre_send_nonce, post_nonce) {
                            (Some(pre), Some(post)) if post > pre => {
                                tracing::warn!(
                                    %from_address,
                                    network = %self.chain.network,
                                    pre_nonce = pre,
                                    post_nonce = post,
                                    error = %error_str,
                                    "Nonce error but confirmed TX count advanced, \
                                     original TX likely mined - skipping retry"
                                );
                                return Err(FacilitatorLocalError::ContractCall(format!(
                                    "Nonce error but TX count advanced \
                                     ({pre} -> {post}), \
                                     original TX may have been mined: {error_str}"
                                )));
                            }
                            (Some(_), Some(_)) => {}
                            _ => {
                                tracing::warn!(
                                    %from_address,
                                    network = %self.chain.network,
                                    error = %error_str,
                                    "Nonce error and the TX count probe was unavailable - \
                                     cannot rule out that the original TX mined, skipping retry"
                                );
                                return Err(FacilitatorLocalError::ContractCall(format!(
                                    "Nonce error and TX count could not be verified, \
                                     original TX may have been mined: {error_str}"
                                )));
                            }
                        }

                        tracing::warn!(
                            attempt = attempt + 1,
                            max_retries = MAX_NONCE_RETRIES,
                            error = %error_str,
                            %from_address,
                            network = %self.chain.network,
                            "Nonce error detected, retrying after backoff"
                        );
                        // Backoff to let the RPC sync its pending state. It is
                        // exponential and jittered because a fixed delay makes
                        // settles that just collided wake up together and
                        // collide again on the same nonce.
                        let backoff_ms = {
                            use rand::Rng as _;
                            let base = 250u64 << attempt.min(3);
                            // Scoped so the non-Send `ThreadRng` is dropped
                            // before the await below.
                            base + rand::thread_rng().gen_range(0..=base / 2)
                        };
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        continue;
                    }

                    return Err(FacilitatorLocalError::ContractCall(error_str));
                }
            }
        }

        unreachable!("retry loop always returns")
    }
}

/// Whether the node rejected the transaction *before* it could enter the
/// mempool, so it provably consumed no nonce on-chain.
///
/// Used to decide whether a reserved nonce can be handed back. Anything not
/// listed here is treated as ambiguous — the transaction may be propagating —
/// and keeps the conservative reset instead.
///
/// `pub(crate)`: `chain/failure.rs` asserts against it that every failure it
/// advertises as retryable is one this function proves never queued. A retry
/// advised for a nonce the allocator did NOT release widens a gap rather than
/// curing it, so the two must be checked together rather than reasoned about
/// separately.
pub(crate) fn is_pre_broadcast_rejection(error: &str) -> bool {
    let lower = error.to_lowercase();
    // A nonce error means the node evaluated our nonce against its own view;
    // the transaction never queued, but the resync path (not release) is the
    // right recovery because our counter is what is wrong.
    if is_nonce_error(&lower) {
        return false;
    }
    lower.contains("execution reverted")
        || lower.contains("gas required exceeds")
        || lower.contains("intrinsic gas too low")
        || lower.contains("exceeds block gas limit")
        || lower.contains("insufficient funds")
        || lower.contains("max fee per gas less than block base fee")
        || is_mempool_full(&lower)
}

/// Whether a failed `eth_sendRawTransaction` may still have put the transaction
/// in a mempool.
///
/// Only a node that answered and refused proves it did not: its JSON-RPC error
/// is a verdict on the transaction -- except `already known`, which says the
/// node holds it. A refusal can also arrive inside a non-2xx HTTP body or a
/// body that is not a well-formed response, so both are read for one. What is
/// left leaves the bytes possibly delivered: a timeout, a dropped connection,
/// a gateway's 5xx, a null or unreadable answer, and retries the transport
/// layer gave up on, since an earlier attempt may have got through. Refused
/// before any node saw it: a connection never made, a request never built or
/// serialized, and an HTTP 4xx without a JSON-RPC verdict (auth, rate limit).
///
/// Checked against [`is_pre_broadcast_rejection`] too, which decides on the
/// same text whether the reserved nonce goes back: a transaction reported as
/// possibly queued must never also have had its nonce handed back.
pub(crate) fn broadcast_may_have_queued(error: &alloy::transports::TransportError) -> bool {
    use alloy::transports::{RpcError, TransportErrorKind};

    /// The `message` of a JSON-RPC error carried as text, bare or in an envelope.
    fn verdict_in(text: &str) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let payload = value.get("error").unwrap_or(&value);
        payload.get("code")?;
        payload
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }
    let holds_it = |message: &str| message.to_ascii_lowercase().contains("already known");

    let queued = match error {
        RpcError::ErrorResp(payload) => holds_it(&payload.message),
        RpcError::DeserError { text, .. } => verdict_in(text).is_none_or(|m| holds_it(&m)),
        RpcError::Transport(TransportErrorKind::HttpError(http)) => match verdict_in(&http.body) {
            Some(message) => holds_it(&message),
            None => http.status >= 500,
        },
        RpcError::Transport(TransportErrorKind::Custom(inner)) => {
            match inner.downcast_ref::<reqwest::Error>() {
                Some(e) => !(e.is_connect() || e.is_builder()),
                None => true,
            }
        }
        RpcError::Transport(_) | RpcError::NullResp => true,
        RpcError::SerError(_) | RpcError::UnsupportedFeature(_) | RpcError::LocalUsageError(_) => {
            false
        }
    };
    queued && !is_pre_broadcast_rejection(&format!("{error:?}"))
}

/// Whether the node refused the transaction because its own mempool has no
/// room for it (geth's `txpool is full: already have N pending transactions
/// in queue`).
///
/// The transaction never reached the mempool, so — same as the other checks
/// in [`is_pre_broadcast_rejection`] — it provably consumed no nonce and the
/// reservation must be handed back, not just reset. Without this, `evm.rs`
/// fell into `reset_nonce`, which keeps the high-water mark pinned on a nonce
/// that will never be mined: every subsequent settle from that signer then
/// resyncs to `high_water + 1`, permanently skipping the hole.
///
/// Matched by MESSAGE, not by the JSON-RPC code: `-32003` is overloaded and
/// also carries `out of gas: gas exhausted during memory expansion` on
/// `eth_call` (see `handlers.rs`'s `OWNER_SCAN_BATCH` doc comment and its
/// `classifies_rpc_failures_as_inconclusive` test) — a real rejection of the
/// call, not evidence the nonce was never consumed. Matching the code would
/// silently release a nonce that MAY already be in flight.
///
/// `pub(crate)`: `handlers.rs` also matches on this (retryable-vs-terminal
/// classification for the HTTP response), and the two must never drift onto
/// separate string lists.
pub(crate) fn is_mempool_full(error: &str) -> bool {
    error.to_lowercase().contains("txpool is full")
}

/// Whether the node rejected the transaction because OUR nonce is ahead of
/// what the chain has: geth's `nonce too high: address 0x.., tx: N state: M`.
///
/// This is the one nonce error that is positive evidence about WHO is wrong.
/// `nonce too low` and `replacement underpriced` mean the chain is at or past
/// our nonce -- something of ours landed, and a high-water mark protecting an
/// in-flight sibling is still meaningful. `nonce too high` means the opposite:
/// the node has no record of the nonces between its state and ours, so the mark
/// is pinned on transactions that will never be mined.
///
/// Separated from [`is_nonce_error`] because the recovery differs. Both retry;
/// only this one may discard the high-water mark. See
/// [`PendingNonceManager::resync_to_chain`] for why that distinction is what
/// keeps a failure burst from wedging the signer permanently.
pub(crate) fn is_nonce_too_high(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("nonce") && lower.contains("too high")
}

/// Check if a transport error is a nonce-related error that can be retried.
///
/// Node phrasings vary more than the original `nonce && ...` conjunction
/// allowed: geth answers a duplicate submission with a bare `already known`
/// (no "nonce" anywhere), several clients shorten the replacement error to
/// `replacement underpriced`, and a nonce left ahead of the chain surfaces as
/// `nonce too high`. All three are recoverable by resyncing and retrying, so
/// each is matched on its own rather than behind the `nonce` guard.
///
/// `pub(crate)`: `chain/failure.rs` classifies HTTP responses on the same
/// phrasings. One list, so a phrasing that earns a retry here cannot fail to
/// earn one there.
pub(crate) fn is_nonce_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("already known")
        || lower.contains("replacement transaction underpriced")
        || lower.contains("replacement underpriced")
        || (lower.contains("nonce")
            && (lower.contains("too low")
                || lower.contains("too high")
                || lower.contains("gap")
                || lower.contains("has already been used")))
}

impl NetworkProviderOps for EvmProvider {
    /// Address of the default signer used by this provider (for tx sending).
    fn signer_address(&self) -> MixedAddress {
        self.inner.default_signer_address().into()
    }

    /// x402 network handled by this provider.
    fn network(&self) -> Network {
        self.chain.network
    }
}

impl FromEnvByNetworkBuild for EvmProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let env_var = from_env::rpc_env_name_from_network(network);
        let rpc_url = match std::env::var(env_var).ok() {
            Some(rpc_url) => rpc_url,
            None => {
                tracing::warn!(network=%network, "no RPC URL configured, skipping");
                return Ok(None);
            }
        };
        let wallet = from_env::SignerType::from_env()?.make_evm_wallet(network)?;
        let is_eip1559 = match network {
            Network::BaseSepolia => true,
            Network::Base => true,
            Network::XdcMainnet => false,
            Network::AvalancheFuji => true,
            Network::Avalanche => true,
            Network::Solana => false,
            Network::SolanaDevnet => false,
            Network::PolygonAmoy => true,
            Network::Polygon => true,
            Network::Optimism => true,
            Network::OptimismSepolia => true,
            Network::Celo => true,
            Network::CeloSepolia => true,
            Network::HyperEvm => true,
            Network::HyperEvmTestnet => true,
            Network::Sei => true,
            Network::SeiTestnet => true,
            Network::Ethereum => true,
            Network::EthereumSepolia => true,
            Network::Arbitrum => true,
            Network::ArbitrumSepolia => true,
            Network::Unichain => true,
            Network::UnichainSepolia => true,
            Network::Monad => true,
            Network::Bsc => true,        // BSC supports EIP-1559 since BEP-95
            Network::SkaleBase => false, // SKALE does NOT support EIP-1559, uses legacy tx
            Network::SkaleBaseSepolia => false, // SKALE does NOT support EIP-1559, uses legacy tx
            Network::Scroll => true,     // Scroll zkEVM supports EIP-1559
            Network::Robinhood => true,  // Arbitrum Orbit: type-2 txs accepted (tips no-op, FCFS)
            Network::RobinhoodTestnet => true,
            // Arc prices type-2 transactions: the latest block carries a
            // baseFeePerGas (20 gwei, measured) and `eth_feeHistory` answers.
            Network::Arc | Network::ArcTestnet => true,
            Network::Near => false,           // NEAR is not an EVM chain
            Network::NearTestnet => false,    // NEAR is not an EVM chain
            #[cfg(feature = "hedera")]
            Network::Hedera | Network::HederaTestnet => false,
            Network::Stellar => false,        // Stellar is not an EVM chain
            Network::StellarTestnet => false, // Stellar is not an EVM chain
            #[cfg(feature = "xrpl")]
            Network::Xrpl => false, // XRPL is not an EVM chain
            #[cfg(feature = "xrpl")]
            Network::XrplTestnet => false, // XRPL is not an EVM chain
            Network::Fogo => false,           // Fogo is a Solana network, not EVM
            Network::FogoTestnet => false,    // Fogo is a Solana network, not EVM
            #[cfg(feature = "algorand")]
            Network::Algorand => false, // Algorand is not an EVM chain
            #[cfg(feature = "algorand")]
            Network::AlgorandTestnet => false, // Algorand is not an EVM chain
            #[cfg(feature = "sui")]
            Network::Sui => false, // Sui is not an EVM chain
            #[cfg(feature = "sui")]
            Network::SuiTestnet => false, // Sui is not an EVM chain
        };
        let provider = EvmProvider::try_new(wallet, &rpc_url, is_eip1559, network).await?;
        // Arc included: what the RPC answers never decides whether a configured
        // network is served. `chain_identity::spawn` compares every EVM RPC's
        // chain id in the background and alerts; `/health/ready` reports a
        // mismatch as `down`. Through 2.39.1 an Arc mismatch was an `Err` that
        // stopped the process, and through 2.39.3 it left Arc out of /supported
        // until the next deploy.
        Ok(Some(provider))
    }
}

impl<P> Facilitator for P
where
    P: MetaEvmProvider + Sync,
    FacilitatorLocalError: From<P::Error>,
{
    type Error = FacilitatorLocalError;

    /// Verify x402 payment intent by simulating signature validity and ERC-3009 transfer.
    ///
    /// For EIP-6492 signatures, perform a multicall: first the validator’s
    /// `isValidSigWithSideEffects` (which *may* deploy the counterfactual wallet in sim),
    /// then the token’s `transferWithAuthorization`. Both run within a single `eth_call`
    /// so the state is shared during simulation.
    ///
    /// # Errors
    /// - [`FacilitatorLocalError::NetworkMismatch`], [`FacilitatorLocalError::SchemeMismatch`], [`FacilitatorLocalError::ReceiverMismatch`] if inputs are inconsistent.
    /// - [`FacilitatorLocalError::InvalidTiming`] if outside `validAfter/validBefore`.
    /// - [`FacilitatorLocalError::InsufficientFunds`] / `FacilitatorLocalError::InsufficientValue` on balance/value checks.
    /// - [`FacilitatorLocalError::ContractCall`] if on-chain calls revert.
    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;
        let (contract, payment, eip712_domain) =
            assert_valid_payment(self.inner(), self.chain(), payload, requirements).await?;

        let signed_message = SignedMessage::extract(&payment, &eip712_domain)?;
        let payer = signed_message.address;
        let hash = signed_message.hash;
        match signed_message.signature {
            StructuredSignature::EIP6492 {
                factory: _,
                factory_calldata: _,
                inner,
                original,
            } => {
                // Prepare the call to validate EIP-6492 signature
                let validator6492 = Validator6492::new(VALIDATOR_ADDRESS, self.inner());
                let is_valid_signature_call =
                    validator6492.isValidSigWithSideEffects(payer, hash, original);
                // Check if the token requires v,r,s signature variant (e.g., PYUSD)
                if requires_vrs_signature(*contract.address()) {
                    // Prepare the call to simulate transfer the funds using v,r,s variant
                    let transfer_call =
                        transferWithAuthorization_1(&contract, &payment, inner).await?;
                    // Execute both calls in a single transaction simulation to accommodate for possible smart wallet creation
                    let (is_valid_signature_result, transfer_result) = self
                        .inner()
                        .multicall()
                        .add(is_valid_signature_call)
                        .add(transfer_call.tx)
                        .aggregate3()
                        .instrument(tracing::info_span!("call_transferWithAuthorization_1",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                v = %transfer_call.v,
                                r = %transfer_call.r,
                                s = %transfer_call.s,
                                token_contract = %transfer_call.contract_address,
                                otel.kind = "client",
                        ))
                        .await
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                    let is_valid_signature_result = is_valid_signature_result
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                    if !is_valid_signature_result {
                        return Err(FacilitatorLocalError::InvalidSignature(
                            payer.into(),
                            "Incorrect signature".to_string(),
                        ));
                    }
                    transfer_result
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;
                } else {
                    // Prepare the call to simulate transfer the funds using compact signature
                    let transfer_call =
                        transferWithAuthorization_0(&contract, &payment, inner).await?;
                    // Execute both calls in a single transaction simulation to accommodate for possible smart wallet creation
                    let (is_valid_signature_result, transfer_result) = self
                        .inner()
                        .multicall()
                        .add(is_valid_signature_call)
                        .add(transfer_call.tx)
                        .aggregate3()
                        .instrument(tracing::info_span!("call_transferWithAuthorization_0",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                signature = %transfer_call.signature,
                                token_contract = %transfer_call.contract_address,
                                otel.kind = "client",
                        ))
                        .await
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                    let is_valid_signature_result = is_valid_signature_result
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                    if !is_valid_signature_result {
                        return Err(FacilitatorLocalError::InvalidSignature(
                            payer.into(),
                            "Incorrect signature".to_string(),
                        ));
                    }
                    transfer_result
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;
                }
            }
            StructuredSignature::EIP1271(signature) => {
                // It is EOA or EIP-1271 signature, which we can pass to the transfer simulation
                // Check if the token requires v,r,s signature variant (e.g., PYUSD)
                if requires_vrs_signature(*contract.address()) {
                    let transfer_call =
                        transferWithAuthorization_1(&contract, &payment, signature).await?;
                    transfer_call
                        .tx
                        .call()
                        .into_future()
                        .instrument(tracing::info_span!("call_transferWithAuthorization_1",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                v = %transfer_call.v,
                                r = %transfer_call.r,
                                s = %transfer_call.s,
                                token_contract = %transfer_call.contract_address,
                                otel.kind = "client",
                        ))
                        .await
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                } else {
                    let transfer_call =
                        transferWithAuthorization_0(&contract, &payment, signature).await?;
                    transfer_call
                        .tx
                        .call()
                        .into_future()
                        .instrument(tracing::info_span!("call_transferWithAuthorization_0",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                signature = %transfer_call.signature,
                                token_contract = %transfer_call.contract_address,
                                otel.kind = "client",
                        ))
                        .await
                        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
                }
            }
        }

        Ok(VerifyResponse::valid(payer.into()))
    }

    /// Settle a verified payment on-chain.
    ///
    /// If the signer is counterfactual (EIP-6492) and the wallet is not yet deployed,
    /// this submits **one** transaction to Multicall3 (`aggregate3`) that:
    /// 1) calls the 6492 factory with the provided calldata (best-effort prepare),
    /// 2) calls `transferWithAuthorization` with the **inner** signature.
    ///
    /// This makes deploy + transfer atomic and avoids read-your-write issues.
    ///
    /// If the wallet is already deployed (or the signature is plain EIP-1271/EOA),
    /// we submit a single `transferWithAuthorization` transaction.
    ///
    /// # Returns
    /// A [`SettleResponse`] containing success flag and transaction hash.
    ///
    /// # Errors
    /// Propagates [`FacilitatorLocalError::ContractCall`] on deployment or transfer failures
    /// and all prior validation errors.
    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;
        let (contract, payment, eip712_domain) =
            assert_valid_payment(self.inner(), self.chain(), payload, requirements).await?;

        let signed_message = SignedMessage::extract(&payment, &eip712_domain)?;
        let payer = signed_message.address;
        let transaction_receipt_fut = match signed_message.signature {
            StructuredSignature::EIP6492 {
                factory,
                factory_calldata,
                inner,
                original: _,
            } => {
                let is_contract_deployed = is_contract_deployed(self.inner(), &payer).await?;
                // Check if the token requires v,r,s signature variant (e.g., PYUSD)
                if requires_vrs_signature(*contract.address()) {
                    let transfer_call =
                        transferWithAuthorization_1(&contract, &payment, inner).await?;
                    if is_contract_deployed {
                        // transferWithAuthorization with v,r,s signature (PYUSD)
                        self.send_transaction(MetaTransaction {
                            authorization_list: None,
                            to: transfer_call.tx.target(),
                            calldata: transfer_call.tx.calldata().clone(),
                            confirmations: 1,
                        })
                        .instrument(
                            tracing::info_span!("call_transferWithAuthorization_1",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                v = %transfer_call.v,
                                r = %transfer_call.r,
                                s = %transfer_call.s,
                                token_contract = %transfer_call.contract_address,
                                sig_kind="EIP6492.deployed.vrs",
                                otel.kind = "client",
                            ),
                        )
                    } else {
                        // deploy the smart wallet, and transferWithAuthorization with v,r,s signature
                        let deployment_call = IMulticall3::Call3 {
                            allowFailure: true,
                            target: factory,
                            callData: factory_calldata,
                        };
                        let transfer_with_authorization_call = IMulticall3::Call3 {
                            allowFailure: false,
                            target: transfer_call.tx.target(),
                            callData: transfer_call.tx.calldata().clone(),
                        };
                        let aggregate_call = IMulticall3::aggregate3Call {
                            calls: vec![deployment_call, transfer_with_authorization_call],
                        };
                        self.send_transaction(MetaTransaction {
                            authorization_list: None,
                            to: MULTICALL3_ADDRESS,
                            calldata: aggregate_call.abi_encode().into(),
                            confirmations: 1,
                        })
                        .instrument(
                            tracing::info_span!("call_transferWithAuthorization_1",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                v = %transfer_call.v,
                                r = %transfer_call.r,
                                s = %transfer_call.s,
                                token_contract = %transfer_call.contract_address,
                                sig_kind="EIP6492.counterfactual.vrs",
                                otel.kind = "client",
                            ),
                        )
                    }
                } else {
                    let transfer_call =
                        transferWithAuthorization_0(&contract, &payment, inner).await?;
                    if is_contract_deployed {
                        // transferWithAuthorization with inner signature
                        self.send_transaction(MetaTransaction {
                            authorization_list: None,
                            to: transfer_call.tx.target(),
                            calldata: transfer_call.tx.calldata().clone(),
                            confirmations: 1,
                        })
                        .instrument(
                            tracing::info_span!("call_transferWithAuthorization_0",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                signature = %transfer_call.signature,
                                token_contract = %transfer_call.contract_address,
                                sig_kind="EIP6492.deployed",
                                otel.kind = "client",
                            ),
                        )
                    } else {
                        // deploy the smart wallet, and transferWithAuthorization with inner signature
                        let deployment_call = IMulticall3::Call3 {
                            allowFailure: true,
                            target: factory,
                            callData: factory_calldata,
                        };
                        let transfer_with_authorization_call = IMulticall3::Call3 {
                            allowFailure: false,
                            target: transfer_call.tx.target(),
                            callData: transfer_call.tx.calldata().clone(),
                        };
                        let aggregate_call = IMulticall3::aggregate3Call {
                            calls: vec![deployment_call, transfer_with_authorization_call],
                        };
                        self.send_transaction(MetaTransaction {
                            authorization_list: None,
                            to: MULTICALL3_ADDRESS,
                            calldata: aggregate_call.abi_encode().into(),
                            confirmations: 1,
                        })
                        .instrument(
                            tracing::info_span!("call_transferWithAuthorization_0",
                                from = %transfer_call.from,
                                to = %transfer_call.to,
                                value = %transfer_call.value,
                                valid_after = %transfer_call.valid_after,
                                valid_before = %transfer_call.valid_before,
                                nonce = %transfer_call.nonce,
                                signature = %transfer_call.signature,
                                token_contract = %transfer_call.contract_address,
                                sig_kind="EIP6492.counterfactual",
                                otel.kind = "client",
                            ),
                        )
                    }
                }
            }
            StructuredSignature::EIP1271(eip1271_signature) => {
                // Check if the token requires v,r,s signature variant (e.g., PYUSD)
                if requires_vrs_signature(*contract.address()) {
                    let transfer_call =
                        transferWithAuthorization_1(&contract, &payment, eip1271_signature).await?;
                    // transferWithAuthorization with v,r,s signature for PYUSD
                    self.send_transaction(MetaTransaction {
                        authorization_list: None,
                        to: transfer_call.tx.target(),
                        calldata: transfer_call.tx.calldata().clone(),
                        confirmations: 1,
                    })
                    .instrument(
                        tracing::info_span!("call_transferWithAuthorization_1",
                            from = %transfer_call.from,
                            to = %transfer_call.to,
                            value = %transfer_call.value,
                            valid_after = %transfer_call.valid_after,
                            valid_before = %transfer_call.valid_before,
                            nonce = %transfer_call.nonce,
                            v = %transfer_call.v,
                            r = %transfer_call.r,
                            s = %transfer_call.s,
                            token_contract = %transfer_call.contract_address,
                            sig_kind="EIP1271.vrs",
                            otel.kind = "client",
                        ),
                    )
                } else {
                    let transfer_call =
                        transferWithAuthorization_0(&contract, &payment, eip1271_signature).await?;
                    // transferWithAuthorization with eip1271 signature
                    self.send_transaction(MetaTransaction {
                        authorization_list: None,
                        to: transfer_call.tx.target(),
                        calldata: transfer_call.tx.calldata().clone(),
                        confirmations: 1,
                    })
                    .instrument(
                        tracing::info_span!("call_transferWithAuthorization_0",
                            from = %transfer_call.from,
                            to = %transfer_call.to,
                            value = %transfer_call.value,
                            valid_after = %transfer_call.valid_after,
                            valid_before = %transfer_call.valid_before,
                            nonce = %transfer_call.nonce,
                            signature = %transfer_call.signature,
                            token_contract = %transfer_call.contract_address,
                            sig_kind="EIP1271",
                            otel.kind = "client",
                        ),
                    )
                }
            }
        };
        let receipt = transaction_receipt_fut.await?;
        let success = receipt.status();
        if success {
            tracing::event!(Level::INFO,
                status = "ok",
                tx = %receipt.transaction_hash,
                "transferWithAuthorization_0 succeeded"
            );

            // Check if ERC-8004 extension is present and create ProofOfPayment
            let proof_of_payment = create_proof_of_payment(
                self.inner(),
                &receipt,
                requirements,
                payload.network,
                payment.from.into(),
                requirements.pay_to.clone(),
                TokenAmount::from(payment.value),
                requirements.asset.clone(),
            )
            .await;

            Ok(SettleResponse {
                success: true,
                error_reason: None,
                payer: payment.from.into(),
                transaction: Some(TransactionHash::Evm(receipt.transaction_hash.0)),
                network: payload.network,
                proof_of_payment,
                extensions: None,
            })
        } else {
            tracing::event!(
                Level::WARN,
                status = "failed",
                tx = %receipt.transaction_hash,
                "transferWithAuthorization_0 failed"
            );
            Ok(SettleResponse {
                success: false,
                error_reason: Some(FacilitatorErrorReason::InvalidScheme),
                payer: payment.from.into(),
                transaction: Some(TransactionHash::Evm(receipt.transaction_hash.0)),
                network: payload.network,
                proof_of_payment: None,
                extensions: None,
            })
        }
    }

    /// Report payment kinds supported by this provider on its current network.
    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let network = self.chain().network();

        // Build list of supported tokens for this network
        let tokens: Vec<SupportedTokenInfo> = exact_payment_tokens(network);

        let extra = if tokens.is_empty() {
            None
        } else {
            Some(SupportedPaymentKindExtra {
                fee_payer: None, // Set at FacilitatorLocal level
                tokens: Some(tokens),
                escrow: None,
            })
        };

        let kinds = vec![SupportedPaymentKind {
            network: network.to_string(),
            x402_version: X402Version::V1,
            scheme: Scheme::Exact,
            network_aliases: None,
            extra,
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}

/// How long a settle waits for the block behind its proof of payment.
///
/// The read happens after the transfer is confirmed, and only when the
/// `8004-reputation` extension asks for a proof and the receipt's logs do not
/// already carry the block timestamp, so this is the most it can add to such a
/// settle. The provider's rate-limit retries run inside it. Past it the settle
/// answers without a proof: the proof is optional, the payment is not.
const PROOF_BLOCK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Create ProofOfPayment if ERC-8004 extension is active.
///
/// Returns Some(ProofOfPayment) if:
/// - The `8004-reputation` extension is present in payment requirements
/// - The network supports ERC-8004 contracts
/// - The include_proof flag is true (default)
/// - The receipt names its block, and that block's timestamp can be read
///
/// `None` in every other case, the last two included: a proof that
/// `verify_payment_facts` is bound to reject is worse than none, because a
/// rating without a proof takes the provisional path while one carrying a bad
/// proof carries a failure. Never an error either -- the transfer is already
/// confirmed and the settle must say so.
#[allow(clippy::too_many_arguments)]
async fn create_proof_of_payment<R: Provider>(
    rpc: &R,
    receipt: &TransactionReceipt,
    requirements: &PaymentRequirements,
    network: Network,
    payer: MixedAddress,
    payee: MixedAddress,
    amount: TokenAmount,
    token: MixedAddress,
) -> Option<ProofOfPayment> {
    // Check if ERC-8004 extension is present
    let extension = Erc8004Extension::from_extra(&requirements.extra)?;

    // Check if proof should be included
    if !extension.include_proof {
        return None;
    }

    // Check if network supports ERC-8004
    if !crate::erc8004::is_erc8004_supported(&network) {
        tracing::debug!(
            network = %network,
            "ERC-8004 extension present but network not supported"
        );
        return None;
    }

    // Block 0 is genesis: a proof naming it fails verification with
    // `proof_block_mismatch`.
    let Some(block_number) = receipt.block_number else {
        tracing::warn!(
            tx = %receipt.transaction_hash,
            network = %network,
            "ERC-8004 proof of payment omitted: the receipt names no block"
        );
        return None;
    };
    // The verifier requires the timestamp of that block, to the second. The
    // facilitator's clock (what this used to send) almost never matches it.
    let timestamp = proof_block_timestamp(rpc, receipt, block_number, network).await?;

    let proof = ProofOfPayment::new(
        TransactionHash::Evm(receipt.transaction_hash.0),
        block_number,
        network,
        payer,
        payee,
        amount,
        token,
        timestamp,
    );

    tracing::info!(
        tx = %receipt.transaction_hash,
        block = block_number,
        "Created ERC-8004 ProofOfPayment"
    );

    Some(proof)
}

/// The timestamp of `block_number`, the block that mined `receipt`.
///
/// Read from the receipt's own logs when the node puts `blockTimestamp` on
/// them, which costs nothing. Measured on 2026-09-15 on the public RPCs of Base,
/// Base Sepolia, Optimism, Arbitrum, Ethereum, Polygon, Celo, BSC, Unichain,
/// Monad and HyperEVM, where it always equalled the block header. No node put
/// the field on the receipt itself.
///
/// Otherwise one `eth_getBlockByNumber`, bounded by
/// [`PROOF_BLOCK_READ_TIMEOUT`]. Avalanche's public RPC omits the field, and the
/// premium endpoints production uses could not be measured. `None`, with a
/// warning, when that read fails, finds no block, or runs out of time.
async fn proof_block_timestamp<R: Provider>(
    rpc: &R,
    receipt: &TransactionReceipt,
    block_number: u64,
    network: Network,
) -> Option<u64> {
    let from_logs = receipt
        .inner
        .logs()
        .iter()
        .filter(|log| log.block_number == Some(block_number))
        .find_map(|log| log.block_timestamp);
    if from_logs.is_some() {
        return from_logs;
    }

    let read = tokio::time::timeout(
        PROOF_BLOCK_READ_TIMEOUT,
        rpc.get_block_by_number(block_number.into()).into_future(),
    )
    .await;
    match read {
        Ok(Ok(Some(block))) => Some(block.header.timestamp),
        Ok(Ok(None)) => {
            tracing::warn!(
                tx = %receipt.transaction_hash,
                block = block_number,
                network = %network,
                "ERC-8004 proof of payment omitted: the node did not return the block"
            );
            None
        }
        Ok(Err(e)) => {
            // Scrubbed: alloy transport errors embed the RPC URL, key included.
            tracing::warn!(
                tx = %receipt.transaction_hash,
                block = block_number,
                network = %network,
                error = %crate::redact::scrub_urls(&e.to_string()),
                "ERC-8004 proof of payment omitted: the block read failed"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                tx = %receipt.transaction_hash,
                block = block_number,
                network = %network,
                timeout_ms = PROOF_BLOCK_READ_TIMEOUT.as_millis() as u64,
                "ERC-8004 proof of payment omitted: the block read timed out"
            );
            None
        }
    }
}

/// A prepared call to `transferWithAuthorization` (ERC-3009) including all derived fields.
///
/// This struct wraps the assembled call builder, making it reusable across verification
/// (`.call()`) and settlement (`.send()`) flows, along with context useful for tracing/logging.
///
/// This is created by [`EvmProvider::transferWithAuthorization_0`].
pub struct TransferWithAuthorization0Call<P> {
    /// The prepared call builder that can be `.call()`ed or `.send()`ed.
    pub tx: SolCallBuilder<P, USDC::transferWithAuthorization_0Call>,
    /// The sender (`from`) address for the authorization.
    pub from: alloy::primitives::Address,
    /// The recipient (`to`) address for the authorization.
    pub to: alloy::primitives::Address,
    /// The amount to transfer (value).
    pub value: U256,
    /// Start of the validity window (inclusive).
    pub valid_after: U256,
    /// End of the validity window (exclusive).
    pub valid_before: U256,
    /// 32-byte authorization nonce (prevents replay).
    pub nonce: FixedBytes<32>,
    /// EIP-712 signature for the transfer authorization.
    pub signature: Bytes,
    /// Address of the token contract used for this transfer.
    pub contract_address: alloy::primitives::Address,
}

/// Validates that a payment authorization is within its `validAfter`/`validBefore`
/// window, per EIP-3009 (an authorization is usable while
/// `validAfter <= block.timestamp < validBefore`).
///
/// Validity is evaluated at `now + CLOCK_SKEW_GRACE_SECS` — a small forward
/// look-ahead rather than at the bare wall-clock instant. This single offset
/// serves two purposes:
/// - it tolerates buyers whose clock is slightly ahead, accepting auths whose
///   `validAfter` is up to `CLOCK_SKEW_GRACE_SECS` in the future; and
/// - it keeps a settlement-latency safety margin on the expiry side: an auth is
///   rejected once it has fewer than `CLOCK_SKEW_GRACE_SECS` of validity left
///   (`valid_before < now + grace`), so a settlement tx submitted now cannot
///   expire on-chain before it is mined.
///
/// This is deliberately *stricter* than the raw spec on the expiry side (it
/// demands a small buffer before `validBefore` rather than accepting auths that
/// just barely expired). It never accepts an expired or not-yet-active auth, so
/// there is no timing-bypass or replay risk. It is also the regression guard for
/// the historical inverted-comparison bug that rejected valid `validAfter`-in-
/// the-past auths and accepted `validAfter`-in-the-future ones.
///
/// # Errors
/// Returns [`FacilitatorLocalError::InvalidTiming`] if the authorization is not yet active or already expired.
/// Returns [`FacilitatorLocalError::ClockError`] if the system clock cannot be read.
#[instrument(skip_all, err)]
fn assert_time(
    payer: MixedAddress,
    valid_after: UnixTimestamp,
    valid_before: UnixTimestamp,
) -> Result<(), FacilitatorLocalError> {
    const CLOCK_SKEW_GRACE_SECS: u64 = 6;
    let now = UnixTimestamp::try_now().map_err(FacilitatorLocalError::ClockError)?;
    if valid_before < now + CLOCK_SKEW_GRACE_SECS {
        return Err(FacilitatorLocalError::InvalidTiming(
            payer,
            format!(
                "Expired: now + grace {} > valid_before {}",
                now + CLOCK_SKEW_GRACE_SECS,
                valid_before
            ),
        ));
    }
    if valid_after > now + CLOCK_SKEW_GRACE_SECS {
        return Err(FacilitatorLocalError::InvalidTiming(
            payer,
            format!(
                "Not active yet: valid_after {valid_after} > now + grace {}",
                now + CLOCK_SKEW_GRACE_SECS
            ),
        ));
    }
    Ok(())
}

/// Checks if the payer has enough on-chain token balance to meet the `maxAmountRequired`.
///
/// Performs an `ERC20.balanceOf()` call using the USDC contract instance.
///
/// # Errors
/// Returns [`FacilitatorLocalError::InsufficientFunds`] if the balance is too low.
/// Returns [`FacilitatorLocalError::ContractCall`] if the balance query fails.
#[instrument(skip_all, err, fields(
    sender = %sender,
    max_required = %max_amount_required,
    token_contract = %usdc_contract.address()
))]
async fn assert_enough_balance<P: Provider>(
    usdc_contract: &USDC::USDCInstance<P>,
    sender: &EvmAddress,
    max_amount_required: U256,
) -> Result<(), FacilitatorLocalError> {
    let balance = usdc_contract
        .balanceOf(sender.0)
        .call()
        .into_future()
        .instrument(tracing::info_span!(
            "fetch_token_balance",
            token_contract = %usdc_contract.address(),
            sender = %sender,
            otel.kind = "client"
        ))
        .await
        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;

    if balance < max_amount_required {
        Err(FacilitatorLocalError::InsufficientFunds((*sender).into()))
    } else {
        Ok(())
    }
}

/// Verifies that the declared `value` in the payload is sufficient for the required amount.
///
/// This is a static check (not on-chain) that compares two numbers.
///
/// # Errors
/// Return [`FacilitatorLocalError::InsufficientValue`] if the payload's value is less than required.
#[instrument(skip_all, err, fields(
    sent = %sent,
    max_amount_required = %max_amount_required
))]
fn assert_enough_value(
    payer: &EvmAddress,
    sent: &U256,
    max_amount_required: &U256,
) -> Result<(), FacilitatorLocalError> {
    if sent < max_amount_required {
        Err(FacilitatorLocalError::InsufficientValue((*payer).into()))
    } else {
        Ok(())
    }
}

/// Check whether contract code is present at `address`.
///
/// Uses `eth_getCode` against this provider. This is useful after a counterfactual
/// deployment to confirm visibility on the sending RPC before submitting a
/// follow-up transaction.
///
/// # Errors
/// Return [`FacilitatorLocalError::ContractCall`] if the RPC call fails.
async fn is_contract_deployed<P: Provider>(
    provider: P,
    address: &Address,
) -> Result<bool, FacilitatorLocalError> {
    let bytes = provider
        .get_code_at(*address)
        .into_future()
        .instrument(tracing::info_span!("get_code_at",
            address = %address,
            otel.kind = "client",
        ))
        .await
        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
    Ok(!bytes.is_empty())
}

/// Constructs the correct EIP-712 domain for signature verification.
///
/// Resolves the `name` and `version` with this priority:
/// 1. Static metadata from known token deployments (USDC, EURC, AUSD, PYUSD) - TRUSTED
/// 2. Client-provided `extra.name`/`extra.version` (for unknown tokens only)
/// 3. On-chain `name()`/`version()` calls (fallback)
#[instrument(skip_all, err, fields(
    network = %payload.network,
    asset = %asset_address
))]
async fn assert_domain<P: Provider>(
    chain: &EvmChain,
    token_contract: &USDC::USDCInstance<P>,
    payload: &PaymentPayload,
    asset_address: &Address,
    requirements: &PaymentRequirements,
) -> Result<Eip712Domain, FacilitatorLocalError> {
    // Try to find EIP-712 metadata from known token deployments.
    // IMPORTANT: For known tokens, our verified static config takes priority over
    // client-provided extra.name/extra.version because clients may send incorrect
    // domain info (e.g., version "1" for Avalanche USDC which actually uses "2").
    // The EIP-712 domain MUST match what the on-chain contract expects.
    let known_eip712 = find_known_eip712_metadata(payload.network, asset_address);

    let name = if let Some((ref known_name, _)) = known_eip712 {
        // Known token: use our verified static metadata
        if let Some(client_name) = requirements
            .extra
            .as_ref()
            .and_then(|e| e.get("name")?.as_str())
        {
            if client_name != known_name {
                tracing::warn!(
                    client_name,
                    static_name = %known_name,
                    "Client EIP-712 name differs from static config, using static"
                );
            }
        }
        known_name.clone()
    } else if let Some(name) = requirements
        .extra
        .as_ref()
        .and_then(|e| e.get("name")?.as_str().map(str::to_string))
    {
        // Unknown token: use client-provided name
        name
    } else {
        // Fallback: query on-chain
        token_contract
            .name()
            .call()
            .into_future()
            .instrument(tracing::info_span!(
                "fetch_eip712_name",
                otel.kind = "client",
            ))
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?
    };
    let chain_id = chain.chain_id;
    let version = if let Some((_, ref known_version)) = known_eip712 {
        // Known token: use our verified static metadata
        if let Some(client_version) = requirements
            .extra
            .as_ref()
            .and_then(|e| e.get("version")?.as_str())
        {
            if client_version != known_version {
                tracing::warn!(
                    client_version,
                    static_version = %known_version,
                    "Client EIP-712 version differs from static config, using static"
                );
            }
        }
        known_version.clone()
    } else if let Some(version) = requirements
        .extra
        .as_ref()
        .and_then(|extra| extra.get("version"))
        .and_then(|version| version.as_str().map(|s| s.to_string()))
    {
        // Unknown token: use client-provided version
        version
    } else {
        // Fallback: query on-chain
        token_contract
            .version()
            .call()
            .into_future()
            .instrument(tracing::info_span!(
                "fetch_eip712_version",
                otel.kind = "client",
            ))
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?
    };
    let domain = eip712_domain! {
        name: name,
        version: version,
        chain_id: chain_id,
        verifying_contract: *asset_address,
    };
    Ok(domain)
}

/// Find EIP-712 metadata (name, version) for a known token deployment.
///
/// Checks all supported stablecoin deployments (USDC, EURC, AUSD, PYUSD)
/// and returns the EIP-712 domain name and version if the asset address matches.
/// Public so DX402 can derive the same EIP-712 digest the payer signed without
/// reimplementing domain resolution. Duplicating it would be a real hazard:
/// the domain name differs per chain and even flips between a chain's mainnet
/// and testnet (Base mainnet is `"USD Coin"`, Base Sepolia is `"USDC"`), so a
/// second copy would drift and silently recover the wrong public key.
pub fn find_known_eip712_metadata(
    network: Network,
    asset_address: &Address,
) -> Option<(String, String)> {
    let asset_mixed: MixedAddress = (*asset_address).into();

    // Check USDC
    if let Some(usdc) = USDCDeployment::by_network(network) {
        if usdc.address() == asset_mixed {
            if let Some(eip712) = &usdc.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check EURC
    if let Some(eurc) = EURCDeployment::by_network(network) {
        if eurc.address() == asset_mixed {
            if let Some(eip712) = &eurc.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check AUSD
    if let Some(ausd) = AUSDDeployment::by_network(network) {
        if ausd.address() == asset_mixed {
            if let Some(eip712) = &ausd.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check PYUSD
    if let Some(pyusd) = PYUSDDeployment::by_network(network) {
        if pyusd.address() == asset_mixed {
            if let Some(eip712) = &pyusd.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check USDT (USDT0 omnichain stablecoin)
    if let Some(usdt) = USDTDeployment::by_network(network) {
        if usdt.address() == asset_mixed {
            if let Some(eip712) = &usdt.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check USDG (Global Dollar by Paxos). The static entry is mandatory here:
    // USDG's on-chain version() getter reverts, so the RPC fallback in
    // assert_domain can never resolve this token's domain.
    if let Some(usdg) = USDGDeployment::by_network(network) {
        if usdg.address() == asset_mixed {
            if let Some(eip712) = &usdg.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    None
}

/// Runs all preconditions needed for a successful payment:
/// - Valid scheme, network, and receiver.
/// - Valid time window (validAfter/validBefore).
/// - Correct EIP-712 domain construction.
/// - Sufficient on-chain balance.
/// - Sufficient value in payload.
#[instrument(skip_all, err)]
async fn assert_valid_payment<P: Provider>(
    provider: P,
    chain: &EvmChain,
    payload: &PaymentPayload,
    requirements: &PaymentRequirements,
) -> Result<(USDC::USDCInstance<P>, ExactEvmPayment, Eip712Domain), FacilitatorLocalError> {
    let payment_payload = match &payload.payload {
            #[cfg(feature = "hedera")]
            ExactPaymentPayload::Hedera(_) => return Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        ExactPaymentPayload::Evm(payload) => payload,
        ExactPaymentPayload::Solana(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
        ExactPaymentPayload::Near(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
        ExactPaymentPayload::Stellar(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
        #[cfg(feature = "algorand")]
        ExactPaymentPayload::Algorand(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
        #[cfg(feature = "sui")]
        ExactPaymentPayload::Sui(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
        ExactPaymentPayload::SolanaSettlementAccount(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
        #[cfg(feature = "xrpl")]
        ExactPaymentPayload::Xrpl(_) => {
            return Err(FacilitatorLocalError::UnsupportedNetwork(None));
        }
    };
    let payer = payment_payload.authorization.from;
    if payload.network != chain.network {
        return Err(FacilitatorLocalError::NetworkMismatch(
            Some(payer.into()),
            chain.network,
            payload.network,
        ));
    }
    if requirements.network != chain.network {
        return Err(FacilitatorLocalError::NetworkMismatch(
            Some(payer.into()),
            chain.network,
            requirements.network,
        ));
    }
    if payload.scheme != requirements.scheme {
        return Err(FacilitatorLocalError::SchemeMismatch(
            Some(payer.into()),
            requirements.scheme,
            payload.scheme,
        ));
    }
    let payload_to: EvmAddress = payment_payload.authorization.to;
    let requirements_to: EvmAddress = requirements
        .pay_to
        .clone()
        .try_into()
        .map_err(|e| FacilitatorLocalError::InvalidAddress(format!("{e:?}")))?;
    if payload_to != requirements_to {
        return Err(FacilitatorLocalError::ReceiverMismatch(
            payer.into(),
            payload_to.to_string(),
            requirements_to.to_string(),
        ));
    }
    let valid_after = payment_payload.authorization.valid_after;
    let valid_before = payment_payload.authorization.valid_before;
    assert_time(payer.into(), valid_after, valid_before)?;

    // B6: strict asset allow-list. Only assets declared in `src/network.rs`
    // (USDC, EURC, AUSD, PYUSD, USDT deployments) are accepted on each
    // network. Refuses arbitrary ERC-20s before any RPC call so a hostile
    // payload cannot trick the facilitator into invoking
    // `transferWithAuthorization` on a token we did not pre-approve.
    crate::chain::assert_supported_asset(chain.network, Some(payer.into()), &requirements.asset)?;

    let asset_address = requirements
        .asset
        .clone()
        .try_into()
        .map_err(|e| FacilitatorLocalError::InvalidAddress(format!("{e:?}")))?;
    let contract = USDC::new(asset_address, provider);

    let domain = assert_domain(chain, &contract, payload, &asset_address, requirements).await?;

    // F4 (EIP-2): reject signatures in non-canonical (high-s) form.
    //
    // secp256k1 has two valid `s` values for every signature: `s` and `n - s`,
    // where `n` is the curve group order. An attacker can flip one valid
    // signature into the other by negating `s`, producing a second valid
    // signature over the same authorization — classic ECDSA malleability.
    // EIP-3009 nonces prevent replay of the *payload*, so the practical blast
    // radius here is bounded, but enforcing the EIP-2 canonical form
    // (`s <= n/2`) keeps the facilitator inside Ethereum's normalised
    // signature space and is cheap defense-in-depth.
    //
    // Layout: signature is r (32) || s (32) || v (1) = 65 bytes.
    //
    // Constants (secp256k1):
    //   N   = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFE BAAEDCE6 AF48A03B BFD25E8C D0364141
    //   N/2 = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF 5D576E73 57A4501D DFE92F46 681B20A0
    //
    // Big-endian byte comparison matches numeric comparison, so we compare
    // the raw `s` bytes against the constant directly.
    //
    // BEFORE that rule, the chain's signature schemes, because the 65-byte rule
    // would otherwise answer for them. An EIP-6492 envelope is never 65 bytes,
    // so on a chain with no validator it would come back as "wrong length" --
    // true, and useless: the caller goes looking for a malformed signature
    // instead of reading that the scheme is not served there.
    assert_signature_scheme_supported(chain.network, payer, &payment_payload.signature)?;
    {
        let sig_bytes = &payment_payload.signature.0;
        if sig_bytes.len() != 65 {
            return Err(FacilitatorLocalError::InvalidSignature(
                payer.into(),
                format!(
                    "invalid_signature_length: expected 65, got {}",
                    sig_bytes.len()
                ),
            ));
        }
        const SECP256K1_N_HALF: [u8; 32] = [
            0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46,
            0x68, 0x1B, 0x20, 0xA0,
        ];
        let s_bytes: [u8; 32] = sig_bytes[32..64].try_into().map_err(|_| {
            FacilitatorLocalError::InvalidSignature(
                payer.into(),
                "invalid_signature_s_slice".to_string(),
            )
        })?;
        if s_bytes > SECP256K1_N_HALF {
            return Err(FacilitatorLocalError::InvalidSignature(
                payer.into(),
                "non_canonical_signature_high_s".to_string(),
            ));
        }
    }

    let amount_required = requirements.max_amount_required.0;
    assert_enough_balance(
        &contract,
        &payment_payload.authorization.from,
        amount_required,
    )
    .await?;
    let value: U256 = payment_payload.authorization.value.into();
    assert_enough_value(&payer, &value, &amount_required)?;

    let payment = ExactEvmPayment {
        chain: *chain,
        from: payment_payload.authorization.from,
        to: payment_payload.authorization.to,
        value: payment_payload.authorization.value,
        valid_after: payment_payload.authorization.valid_after,
        valid_before: payment_payload.authorization.valid_before,
        nonce: payment_payload.authorization.nonce,
        signature: payment_payload.signature.clone(),
    };

    // Arc's initial payment rail supports EOA authorizations only. Recover the
    // signer locally under this chain's USDC domain, before any gas estimate or
    // broadcast. A permissive/misconfigured RPC must not turn a signature for
    // the other Arc network into a sponsored transaction. Other chains keep
    // their existing contract-wallet verification paths.
    if matches!(chain.network, Network::Arc | Network::ArcTestnet) {
        let signed = SignedMessage::extract(&payment, &domain)?;
        let recovered = alloy::primitives::Signature::try_from(payment.signature.0.as_slice())
            .ok()
            .and_then(|signature| signature.recover_address_from_prehash(&signed.hash).ok());
        if recovered != Some(signed.address) {
            return Err(FacilitatorLocalError::InvalidSignature(
                payer.into(),
                "Arc requires an EOA signature for this network's USDC domain".into(),
            ));
        }
    }

    Ok((contract, payment, domain))
}

/// Constructs a full `transferWithAuthorization` call for a verified payment payload.
///
/// This function prepares the transaction builder with gas pricing adapted to the network's
/// capabilities (EIP-1559 or legacy) and packages it together with signature metadata
/// into a [`TransferWithAuthorization0Call`] structure.
///
/// This function does not perform any validation — it assumes inputs are already checked.
#[allow(non_snake_case)]
async fn transferWithAuthorization_0<'a, P: Provider>(
    contract: &'a USDC::USDCInstance<P>,
    payment: &ExactEvmPayment,
    signature: Bytes,
) -> Result<TransferWithAuthorization0Call<&'a P>, FacilitatorLocalError> {
    let from: Address = payment.from.into();
    let to: Address = payment.to.into();
    let value: U256 = payment.value.into();
    let valid_after: U256 = payment.valid_after.into();
    let valid_before: U256 = payment.valid_before.into();
    let nonce = FixedBytes(payment.nonce.0);
    let tx = contract.transferWithAuthorization_0(
        from,
        to,
        value,
        valid_after,
        valid_before,
        nonce,
        signature.clone(),
    );
    Ok(TransferWithAuthorization0Call {
        tx,
        from,
        to,
        value,
        valid_after,
        valid_before,
        nonce,
        signature,
        contract_address: *contract.address(),
    })
}

/// PYUSD contract address on Ethereum mainnet.
/// PYUSD uses Paxos implementation which ONLY supports the v,r,s signature variant
/// (not the compact bytes signature variant like Circle's USDC/EURC).
const PYUSD_ETHEREUM_ADDRESS: Address = address!("6c3ea9036406852006290770BEdFcAbA0e23A0e8");

/// Check if a token contract requires the v,r,s signature variant for transferWithAuthorization.
///
/// PYUSD (PayPal USD) uses Paxos implementation which only supports the v,r,s variant,
/// unlike Circle's implementation (USDC, EURC) which supports both variants.
fn requires_vrs_signature(contract_address: Address) -> bool {
    contract_address == PYUSD_ETHEREUM_ADDRESS
}

/// Upper bound of the canonical (low) `s` value for secp256k1 signatures: `N / 2`.
///
/// `N` is the order of the secp256k1 curve. A signature with `s > N/2` is the
/// arithmetic complement of an otherwise valid signature for the same digest
/// and is therefore malleable: an attacker observing the original signature
/// can produce a different but equally valid 65-byte signature for the same
/// `transferWithAuthorization` payload, defeating naive replay-protection
/// schemes that key on the signature bytes.
///
/// We reject high-s signatures at the facilitator boundary. This is the same
/// rule applied by go-ethereum, OpenZeppelin's ECDSA library, and EIP-2.
const SECP256K1_N_HALF: FixedBytes<32> = FixedBytes::new([
    0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46, 0x68, 0x1B, 0x20, 0xA0,
]);

/// Split a 65-byte signature into its v, r, s components.
///
/// Standard Ethereum signatures are 65 bytes: r (32 bytes) + s (32 bytes) + v (1 byte).
///
/// Enforces canonical (low) `s` per EIP-2 to prevent signature malleability.
///
/// # Errors
/// Returns an error if the signature is not exactly 65 bytes or if `s > N/2`.
fn split_signature(
    signature: &Bytes,
) -> Result<(u8, FixedBytes<32>, FixedBytes<32>), FacilitatorLocalError> {
    if signature.len() != 65 {
        return Err(FacilitatorLocalError::InvalidSignature(
            EvmAddress(Address::ZERO).into(),
            format!(
                "Invalid signature length: expected 65 bytes, got {}",
                signature.len()
            ),
        ));
    }

    let r: FixedBytes<32> = FixedBytes::from_slice(&signature[0..32]);
    let s: FixedBytes<32> = FixedBytes::from_slice(&signature[32..64]);
    let v: u8 = signature[64];

    if s > SECP256K1_N_HALF {
        return Err(FacilitatorLocalError::InvalidSignature(
            EvmAddress(Address::ZERO).into(),
            "non-canonical signature: s > N/2 (EIP-2 malleability)".to_string(),
        ));
    }

    Ok((v, r, s))
}

/// A prepared call to `transferWithAuthorization` (ERC-3009) with v,r,s signature components.
///
/// This struct is used for tokens like PYUSD that only support the v,r,s signature variant.
pub struct TransferWithAuthorization1Call<P> {
    /// The prepared call builder that can be `.call()`ed or `.send()`ed.
    pub tx: SolCallBuilder<P, USDC::transferWithAuthorization_1Call>,
    /// The sender (`from`) address for the authorization.
    pub from: alloy::primitives::Address,
    /// The recipient (`to`) address for the authorization.
    pub to: alloy::primitives::Address,
    /// The amount to transfer (value).
    pub value: U256,
    /// Start of the validity window (inclusive).
    pub valid_after: U256,
    /// End of the validity window (exclusive).
    pub valid_before: U256,
    /// 32-byte authorization nonce (prevents replay).
    pub nonce: FixedBytes<32>,
    /// The v component of the signature.
    pub v: u8,
    /// The r component of the signature.
    pub r: FixedBytes<32>,
    /// The s component of the signature.
    pub s: FixedBytes<32>,
    /// Address of the token contract used for this transfer.
    pub contract_address: alloy::primitives::Address,
}

/// Constructs a `transferWithAuthorization` call using the v,r,s signature variant.
///
/// This is used for tokens like PYUSD (Paxos implementation) that only support
/// the v,r,s signature variant instead of the compact bytes signature.
#[allow(non_snake_case)]
async fn transferWithAuthorization_1<'a, P: Provider>(
    contract: &'a USDC::USDCInstance<P>,
    payment: &ExactEvmPayment,
    signature: Bytes,
) -> Result<TransferWithAuthorization1Call<&'a P>, FacilitatorLocalError> {
    let from: Address = payment.from.into();
    let to: Address = payment.to.into();
    let value: U256 = payment.value.into();
    let valid_after: U256 = payment.valid_after.into();
    let valid_before: U256 = payment.valid_before.into();
    let nonce = FixedBytes(payment.nonce.0);

    // Split the 65-byte signature into v, r, s components
    let (v, r, s) = split_signature(&signature)?;

    let tx = contract.transferWithAuthorization_1(
        from,
        to,
        value,
        valid_after,
        valid_before,
        nonce,
        v,
        r,
        s,
    );
    Ok(TransferWithAuthorization1Call {
        tx,
        from,
        to,
        value,
        valid_after,
        valid_before,
        nonce,
        v,
        r,
        s,
        contract_address: *contract.address(),
    })
}

/// A structured representation of an Ethereum signature.
///
/// This enum normalizes two supported cases:
///
/// - **EIP-6492 wrapped signatures**: used for counterfactual contract wallets.
///   They include deployment metadata (factory + calldata) plus the inner
///   signature that the wallet contract will validate after deployment.
/// - **EIP-1271 signatures**: plain contract (or EOA-style) signatures.
#[derive(Debug, Clone)]
enum StructuredSignature {
    /// An EIP-6492 wrapped signature.
    EIP6492 {
        /// Factory contract that can deploy the wallet deterministically
        factory: alloy::primitives::Address,
        /// Calldata to invoke on the factory (often a CREATE2 deployment).
        factory_calldata: Bytes,
        /// Inner signature for the wallet itself, probably EIP-1271.
        inner: Bytes,
        /// Full original bytes including the 6492 wrapper and magic bytes suffix.
        original: Bytes,
    },
    /// A plain EIP-1271 or EOA signature (no 6492 wrappers).
    EIP1271(Bytes),
}

/// Canonical data required to verify a signature.
#[derive(Debug, Clone)]
struct SignedMessage {
    /// Expected signer (an EOA or contract wallet).
    address: alloy::primitives::Address,
    /// 32-byte digest that was signed (typically an EIP-712 hash).
    hash: FixedBytes<32>,
    /// Structured signature, either EIP-6492 or EIP-1271.
    signature: StructuredSignature,
}

impl SignedMessage {
    /// Construct a [`SignedMessage`] from an [`ExactEvmPayment`] and its
    /// corresponding [`Eip712Domain`].
    ///
    /// This helper ties together:
    /// - The **payment intent** (an ERC-3009 `TransferWithAuthorization` struct),
    /// - The **EIP-712 domain** used for signing,
    /// - And the raw signature bytes attached to the payment.
    ///
    /// Steps performed:
    /// 1. Build an in-memory [`TransferWithAuthorization`] struct from the
    ///    `ExactEvmPayment` fields (`from`, `to`, `value`, validity window, `nonce`).
    /// 2. Compute the **EIP-712 struct hash** for that transfer under the given
    ///    `domain`. This becomes the `hash` field of the signed message.
    /// 3. Parse the raw signature bytes into a [`StructuredSignature`], which
    ///    distinguishes between:
    ///    - EIP-1271 (plain signature), and
    ///    - EIP-6492 (counterfactual signature wrapper).
    /// 4. Assemble all parts into a [`SignedMessage`] and return it.
    ///
    /// # Errors
    ///
    /// Returns [`FacilitatorLocalError`] if:
    /// - The raw signature cannot be decoded as either EIP-1271 or EIP-6492.
    pub fn extract(
        payment: &ExactEvmPayment,
        domain: &Eip712Domain,
    ) -> Result<Self, FacilitatorLocalError> {
        let transfer_with_authorization = TransferWithAuthorization {
            from: payment.from.0,
            to: payment.to.0,
            value: payment.value.into(),
            validAfter: payment.valid_after.into(),
            validBefore: payment.valid_before.into(),
            nonce: FixedBytes(payment.nonce.0),
        };
        let eip712_hash = transfer_with_authorization.eip712_signing_hash(domain);
        let expected_address = payment.from;
        let structured_signature: StructuredSignature = payment.signature.clone().try_into()?;
        let signed_message = Self {
            address: expected_address.into(),
            hash: eip712_hash,
            signature: structured_signature,
        };
        Ok(signed_message)
    }
}

/// The fixed 32-byte magic suffix defined by [EIP-6492](https://eips.ethereum.org/EIPS/eip-6492).
///
/// Any signature ending with this constant is treated as a 6492-wrapped
/// signature; the preceding bytes are ABI-decoded as `(address factory, bytes factoryCalldata, bytes innerSig)`.
const EIP6492_MAGIC_SUFFIX: [u8; 32] =
    hex!("6492649264926492649264926492649264926492649264926492649264926492");

sol! {
    /// Solidity-compatible struct for decoding the prefix of an EIP-6492 signature.
    ///
    /// Matches the tuple `(address factory, bytes factoryCalldata, bytes innerSig)`.
    #[derive(Debug)]
    struct Sig6492 {
        address factory;
        bytes   factoryCalldata;
        bytes   innerSig;
    }
}

impl TryFrom<EvmSignature> for StructuredSignature {
    type Error = FacilitatorLocalError;
    /// Convert from an `EvmSignature` wrapper to a structured signature.
    ///
    /// This delegates to the `TryFrom<Vec<u8>>` implementation.
    fn try_from(signature: EvmSignature) -> Result<Self, Self::Error> {
        signature.0.try_into()
    }
}

impl TryFrom<Vec<u8>> for StructuredSignature {
    type Error = FacilitatorLocalError;

    /// Parse raw signature bytes into a `StructuredSignature`.
    ///
    /// Rules:
    /// - If the last 32 bytes equal [`EIP6492_MAGIC_SUFFIX`], the prefix is
    ///   decoded as a [`Sig6492`] struct and returned as
    ///   [`StructuredSignature::EIP6492`].
    /// - Otherwise, the bytes are returned as [`StructuredSignature::EIP1271`].
    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let is_eip6492 = bytes.len() >= 32 && bytes[bytes.len() - 32..] == EIP6492_MAGIC_SUFFIX;
        let signature = if is_eip6492 {
            let body = &bytes[..bytes.len() - 32];
            let sig6492 = Sig6492::abi_decode_params(body).map_err(|e| {
                FacilitatorLocalError::ContractCall(format!(
                    "Failed to decode EIP6492 signature: {e}"
                ))
            })?;
            StructuredSignature::EIP6492 {
                factory: sig6492.factory,
                factory_calldata: sig6492.factoryCalldata,
                inner: sig6492.innerSig,
                original: bytes.into(),
            }
        } else {
            StructuredSignature::EIP1271(bytes.into())
        };
        Ok(signature)
    }
}

/// A nonce manager that caches nonces locally and checks pending transactions on initialization.
///
/// This implementation attempts to improve upon Alloy's `CachedNonceManager` by using `.pending()` when
/// fetching the initial nonce, which includes pending transactions in the mempool. This prevents
/// "nonce too low" errors when the application restarts while transactions are still pending.
///
/// # How it works
///
/// - **First call for an address**: Fetches the nonce using `.pending()`, which includes
///   transactions in the mempool, not just confirmed transactions.
/// - **Subsequent calls**: Increments the cached nonce locally without querying the RPC.
/// - **Per-address tracking**: Each address has its own cached nonce, allowing concurrent
///   transaction submission from multiple addresses.
///
/// # Thread Safety
///
/// The nonce cache is shared across all clones using `Arc<DashMap>`, ensuring that concurrent
/// requests see consistent nonce values. Each address's nonce is protected by its own `Mutex`
/// to prevent race conditions during allocation.
/// ```
#[derive(Clone, Debug, Default)]
pub struct PendingNonceManager {
    /// Cache of nonce state per address, each behind its own mutex.
    nonces: Arc<DashMap<alloy::primitives::Address, Arc<Mutex<NonceState>>>>,
}

/// How long after the last allocation the chain's pending count is trusted
/// unconditionally on a resync.
///
/// Inside this window a transaction this process allocated may still be
/// propagating, and an RPC load balancer can route the resync to a node that
/// has not seen it yet — so the resync must not rewind below the high-water
/// mark. Past it, anything we allocated has been mined or dropped, and trusting
/// the chain lets a gap left by a dropped transaction heal instead of wedging
/// the signer behind a nonce that will never be used.
const NONCE_TRUST_CHAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(120);

/// How long the chain must CONTINUOUSLY report a pending count at or below this
/// process's high-water mark before the mark is abandoned anyway.
///
/// [`NONCE_TRUST_CHAIN_AFTER`] releases the mark only during a lull, and under
/// continuous traffic there is never one. That gap is not hypothetical: on
/// 2026-09-10, with a Polygon signer wedged behind an unmineable transaction,
/// every send was being refused by the node before it reached the mempool. Each
/// refusal released its nonce, but a sibling settle had usually taken the next
/// one already, so the rollback declined and the high-water mark ratcheted --
/// 1738 locally against 1557 on the chain, and climbing.
///
/// Nothing broke while the signer was frozen, because none of those allocations
/// reached a pool. It breaks on RECOVERY: the moment sends are accepted again,
/// this process would start at 1738 and leave 1557..1737 empty -- a real nonce
/// gap, which is the one failure mode that cannot heal on its own.
///
/// Five minutes because the reasoning is the same as the other constant's, only
/// anchored to a different clock: a transaction this process allocated and that
/// no node has acknowledged in five minutes is not still propagating.
const NONCE_TRUST_CHAIN_AFTER_DRIFT: std::time::Duration = std::time::Duration::from_secs(300);

/// Decides what a resync should hand out, given the chain's own pending count
/// and this process's bookkeeping for the address.
///
/// Extracted out of [`PendingNonceManager::get_next_nonce`] so the decision has
/// exactly one implementation. It used to be duplicated verbatim into a
/// `resync_nonce` test helper — a trap documented in
/// `docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md`: editing
/// one copy and not the other leaves the tests green against the OLD logic.
fn resync_target(
    pending: u64,
    high_water: Option<u64>,
    last_allocated: Option<std::time::Instant>,
    chain_behind_for: Option<std::time::Duration>,
) -> u64 {
    match (high_water, last_allocated) {
        // Nothing we allocated can still be in flight: trust the chain so a
        // gap left by a dropped transaction heals.
        (_, Some(last)) if last.elapsed() >= NONCE_TRUST_CHAIN_AFTER => pending,
        // The chain has been reporting a lower count than our bookkeeping for
        // long enough that nothing we allocated can be in flight either --
        // even though traffic never paused long enough for the branch above to
        // fire. Trust the chain and give up the mark, or the drift becomes a
        // real nonce gap the first time sends start being accepted again.
        (Some(_), _)
            if chain_behind_for.is_some_and(|since| since >= NONCE_TRUST_CHAIN_AFTER_DRIFT) =>
        {
            pending
        }
        // A transaction we allocated may still be propagating and this node
        // may not have seen it. Handing back a nonce at or below the
        // high-water mark would try to REPLACE that in-flight transaction
        // instead of queueing behind it, which is exactly the "replacement
        // transaction underpriced" failure reported under concurrent settles.
        (Some(high_water), _) if pending <= high_water => high_water.saturating_add(1),
        _ => pending,
    }
}

/// Per-address nonce bookkeeping.
#[derive(Debug, Default)]
struct NonceState {
    /// Next nonce to hand out, or `None` when this address must be resynced
    /// against the chain before the next allocation.
    next: Option<u64>,
    /// Highest nonce this process has ever handed out for this address. Never
    /// decreases, so a resync cannot reissue a nonce that is still in flight.
    high_water: Option<u64>,
    /// When the last nonce was handed out, used to decide whether anything we
    /// allocated could still be pending.
    last_allocated: Option<std::time::Instant>,
    /// When the chain first started reporting a pending count at or below
    /// [`Self::high_water`], and has done so on every resync since. `None` when
    /// the chain has caught up. See [`NONCE_TRUST_CHAIN_AFTER_DRIFT`].
    chain_behind_since: Option<std::time::Instant>,
}

#[async_trait]
impl NonceManager for PendingNonceManager {
    async fn get_next_nonce<P, N>(
        &self,
        provider: &P,
        address: alloy::primitives::Address,
    ) -> alloy::transports::TransportResult<u64>
    where
        P: Provider<N>,
        N: alloy::network::Network,
    {
        // Locks dashmap internally for a short duration to clone the `Arc`.
        // We also don't want to hold the dashmap lock through the await point below.
        let state = {
            let rm = self
                .nonces
                .entry(address)
                .or_insert_with(|| Arc::new(Mutex::new(NonceState::default())));
            Arc::clone(rm.value())
        };

        let mut state = state.lock().await;

        let next = match state.next {
            Some(next) => {
                tracing::trace!(%address, next, "allocating cached nonce");
                next
            }
            None => {
                // Resync against the chain. `.pending()` includes transactions
                // sitting in the node's mempool, so a restart mid-flight does
                // not reuse a nonce that is already queued.
                tracing::trace!(%address, "resyncing nonce against chain");
                let pending = provider.get_transaction_count(address).pending().await?;

                // Run the drift clock BEFORE deciding, so the decision sees how
                // long this divergence has lasted rather than only that it
                // exists right now.
                if state.high_water.is_some_and(|mark| pending <= mark) {
                    state
                        .chain_behind_since
                        .get_or_insert_with(std::time::Instant::now);
                } else {
                    state.chain_behind_since = None;
                }
                let chain_behind_for = state.chain_behind_since.map(|at| at.elapsed());

                let target = resync_target(
                    pending,
                    state.high_water,
                    state.last_allocated,
                    chain_behind_for,
                );
                if state.high_water.is_some_and(|mark| target <= mark) {
                    // Giving up the mark is worth a line: it means this process
                    // had drifted above the chain, and every nonce between the
                    // two was allocated to a transaction that never landed.
                    tracing::warn!(
                        %address,
                        chain_pending = pending,
                        high_water = ?state.high_water,
                        behind_for_secs = chain_behind_for.map(|d| d.as_secs()),
                        "nonce high-water mark abandoned; resyncing down to the chain"
                    );
                    state.chain_behind_since = None;
                }
                target
            }
        };

        state.next = Some(next.saturating_add(1));
        state.high_water = Some(state.high_water.map_or(next, |hw| hw.max(next)));
        state.last_allocated = Some(std::time::Instant::now());
        Ok(next)
    }
}

impl PendingNonceManager {
    /// Forces the next allocation for `address` to resync against the chain.
    ///
    /// Called when a transaction fails, since we cannot be certain of the actual
    /// on-chain state (the transaction may or may not have reached the mempool).
    ///
    /// The high-water mark is deliberately preserved: other settles from the
    /// same signer may still be in flight, and wiping it would let the refetched
    /// chain nonce rewind underneath them. [`NONCE_TRUST_CHAIN_AFTER`] is what
    /// eventually releases the mark so a genuinely dropped transaction does not
    /// wedge the signer forever.
    /// Hands back a nonce that was reserved but provably never broadcast.
    ///
    /// Rolls back only when nothing else has allocated since — if a sibling
    /// settle took the next nonce, the gap is real and healing it is the
    /// resync path's job. Without this, a reverting payload on the
    /// unauthenticated `/settle` endpoint would advance the shared counter by
    /// one each time and stall every subsequent settle from that signer.
    pub async fn release_nonce(&self, address: Address, nonce: u64) {
        let state = self.nonces.get(&address).map(|r| Arc::clone(r.value()));
        if let Some(state) = state {
            let mut state = state.lock().await;
            if state.next == Some(nonce.saturating_add(1)) {
                state.next = Some(nonce);
                // The high-water mark has to step back with it, or a resync
                // would refuse to reuse a nonce that never reached the network.
                state.high_water = nonce.checked_sub(1);
                // `info!`, not `debug!`: production runs at `info`, and this
                // is the only observable signal that fix #1 (releasing the
                // nonce on `txpool is full`) is actually firing. At `debug!`
                // it is invisible in prod logs — see
                // docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md.
                tracing::info!(%address, nonce, "released unbroadcast nonce");
            } else {
                tracing::debug!(
                    %address,
                    nonce,
                    next = ?state.next,
                    "not releasing nonce: another allocation followed it"
                );
            }
        }
    }

    /// Drops the cached nonce AND the high-water mark, so the next allocation
    /// takes whatever the chain reports.
    ///
    /// Only for `nonce too high`, which is proof the chain is BEHIND our
    /// counter -- see [`is_nonce_too_high`].
    ///
    /// [`reset_nonce`](Self::reset_nonce) deliberately preserves the mark so a
    /// refetched chain nonce cannot rewind underneath an in-flight sibling, and
    /// leans on [`NONCE_TRUST_CHAIN_AFTER`] to release it eventually. That
    /// backstop cannot fire during a sustained failure burst: every failed
    /// attempt allocates a nonce, every allocation refreshes `last_allocated`,
    /// and so the 120-second idle window never elapses. Each failure then
    /// resyncs to `high_water + 1`, which is one PAST the nonce that just
    /// failed -- a ratchet that climbs and never returns.
    ///
    /// Measured in production 2026-09-01, immediately after a restart had
    /// cleared the state: the first failure opened a gap of 1 (tx 1556 against
    /// chain state 1555), one second later the gap was 31, and 65 seconds later
    /// it was 48. Restarting cured it for about ten minutes each time. Two
    /// restarts and two deploys that day all healed it temporarily and none
    /// fixed it.
    ///
    /// The risk of trusting the chain here is a sibling that this node has not
    /// seen yet, which would make both transactions claim one nonce and fail as
    /// `replacement transaction underpriced` -- a RETRYABLE error, and one the
    /// retry loop above already handles. The risk of not trusting it is a signer
    /// wedged until someone notices and restarts the service. That trade is why
    /// this exists.
    pub async fn resync_to_chain(&self, address: Address) {
        let state = self.nonces.get(&address).map(|r| Arc::clone(r.value()));
        if let Some(state) = state {
            let mut state = state.lock().await;
            let discarded = state.high_water;
            state.next = None;
            state.high_water = None;
            // `info!`, not `debug!`: production runs at `info`, and this is the
            // only signal that the ratchet was broken rather than merely
            // survived. The defect it fixes was invisible for hours because the
            // line that would have shown it was `debug!` (same lesson as
            // `release_nonce` and as the p99 incident the same day).
            tracing::info!(
                %address,
                discarded_high_water = ?discarded,
                "nonce is ahead of the chain; discarding the high-water mark and resyncing"
            );
        }
    }

    pub async fn reset_nonce(&self, address: Address) {
        // Clone the `Arc` and drop the dashmap guard BEFORE the await point,
        // exactly as `get_next_nonce` does. A dashmap shard guard (here the read
        // guard from `.get()`) must never be held across `.await`: doing so is a
        // guard-across-await deadlock hazard (a suspended task holding the shard
        // lock can block another task that needs the same shard). This path runs
        // on settlement failure (`is_nonce_error` retry), so the hazard is on the
        // hot path, not just in tests.
        let state = self.nonces.get(&address).map(|r| Arc::clone(r.value()));
        if let Some(state) = state {
            let mut state = state.lock().await;
            state.next = None;
            tracing::debug!(
                %address,
                high_water = ?state.high_water,
                "reset nonce cache, will resync on next use"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    // ---------------------------------------------------------------
    // Estimate before nonce: `send_call_estimated`.
    // ---------------------------------------------------------------

    /// A JSON-RPC endpoint answered by method name, recording every call.
    ///
    /// `eth_estimateGas` answers after a short sleep, the way a real HTTP
    /// round-trip does, and that is what makes the tests below discriminating.
    /// Alloy races gas and nonce inside one `try_join!`: an estimate that
    /// failed on its first poll would end the join before the nonce filler ever
    /// ran, and an unguarded send would look innocent here while it burns
    /// nonces in production.
    #[derive(Clone)]
    struct ScriptedRpc {
        estimate_reverts: bool,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl ScriptedRpc {
        fn new(estimate_reverts: bool) -> Self {
            Self {
                estimate_reverts,
                calls: Default::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// The JSON-RPC response packet, named through the transport future: the
    /// `alloy` facade re-exports `alloy_json_rpc` only under its `json-rpc`
    /// feature, which this crate does not enable.
    trait OkOf {
        type Ok;
    }
    impl<T, E> OkOf for Result<T, E> {
        type Ok = T;
    }
    type ResponsePacket =
        <<alloy::transports::TransportFut<'static> as Future>::Output as OkOf>::Ok;

    // Generic over the request so the packet type never has to be named; the
    // only request it is ever handed is alloy's serialized single request.
    impl<Req: serde::Serialize> tower::Service<Req> for ScriptedRpc {
        type Response = ResponsePacket;
        type Error = alloy::transports::TransportError;
        type Future = alloy::transports::TransportFut<'static>;

        fn poll_ready(
            &mut self,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, packet: Req) -> Self::Future {
            let this = self.clone();
            let req = serde_json::to_value(&packet).expect("request serialises");
            Box::pin(async move {
                let method = req["method"]
                    .as_str()
                    .expect("ScriptedRpc does not script batches")
                    .to_string();
                this.calls.lock().unwrap().push(method.clone());
                let id = req["id"].to_string();
                let outcome = match method.as_str() {
                    "eth_estimateGas" => {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        // Recorded when the answer lands, not when it is asked
                        // for: "was the nonce requested before the estimate
                        // came back" is the whole question the guard answers.
                        this.calls
                            .lock()
                            .unwrap()
                            .push(ESTIMATE_ANSWERED.to_string());
                        if this.estimate_reverts {
                            r#""error":{"code":3,"message":"execution reverted","data":"0x"}"#
                                .to_string()
                        } else {
                            r#""result":"0x5208""#.to_string()
                        }
                    }
                    "eth_getTransactionCount" => r#""result":"0x7""#.to_string(),
                    "eth_chainId" => r#""result":"0x2105""#.to_string(),
                    "eth_sendRawTransaction" => format!(r#""result":"0x{}""#, "ab".repeat(32)),
                    other => {
                        format!(r#""error":{{"code":-32601,"message":"{other} not scripted"}}"#)
                    }
                };
                let body = format!(r#"{{"jsonrpc":"2.0","id":{id},{outcome}}}"#);
                Ok(serde_json::from_str(&body).expect("scripted response parses"))
            })
        }
    }

    /// Marker [`ScriptedRpc`] records once `eth_estimateGas` has answered.
    const ESTIMATE_ANSWERED: &str = "eth_estimateGas answered";

    /// A contract call over the filler stack `EvmProvider` builds, pointed at
    /// [`ScriptedRpc`], plus the address it sends from.
    ///
    /// Legacy pricing (as on SKALE): with `gas_price` set, the estimate is the
    /// only thing the gas filler still asks the node for, so the script does
    /// not have to fake a fee history.
    fn scripted_call(
        rpc: ScriptedRpc,
        nonces: PendingNonceManager,
    ) -> (
        alloy::contract::CallBuilder<impl Provider<AlloyEthereum>, ()>,
        Address,
    ) {
        let signer = alloy::signers::local::PrivateKeySigner::random();
        let from = signer.address();
        let filler = JoinFill::new(
            GasFiller,
            JoinFill::new(
                BlobGasFiller::default(),
                JoinFill::new(NonceFiller::new(nonces), ChainIdFiller::default()),
            ),
        );
        let provider = ProviderBuilder::default()
            .filler(filler)
            .wallet(EthereumWallet::from(signer))
            .connect_client(RpcClient::new(rpc, true));
        let call = alloy::contract::CallBuilder::new_raw(
            provider,
            Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
        )
        .to(address!("0x00000000000000000000000000000000000c0de0"))
        .gas_price(1_000_000_000);
        (call, from)
    }

    /// The regression test for the Monad freeze (2026-08-24): an estimate that
    /// reverts must leave the signer's nonce where it was. Replace the body of
    /// `send_call_estimated` with a bare `call.send()` and this fails on the
    /// first assertion -- the nonce filler reads `eth_getTransactionCount` and
    /// allocates for a transaction that is never broadcast.
    #[tokio::test]
    async fn a_reverting_estimate_consumes_no_nonce() {
        let rpc = ScriptedRpc::new(true);
        let nonces = PendingNonceManager::default();
        let (call, from) = scripted_call(rpc.clone(), nonces.clone());

        let outcome = send_call_estimated(call, Network::Base).await;

        assert!(
            !rpc.calls().iter().any(|m| m == "eth_getTransactionCount"),
            "a nonce was fetched for a call that reverted on estimation: {:?}",
            rpc.calls()
        );
        assert_eq!(
            read_next(&nonces, from).await,
            None,
            "the nonce manager allocated for a call that was never broadcast"
        );
        assert!(
            !rpc.calls().iter().any(|m| m == "eth_sendRawTransaction"),
            "{:?}",
            rpc.calls()
        );
        assert!(
            matches!(outcome, Err(EstimatedSendError::Reverted(_))),
            "got {:?}",
            outcome.map(|_| ())
        );
    }

    /// The guard must not cost the happy path its send: the estimate has
    /// ANSWERED before the nonce is even requested, then the broadcast -- and
    /// the filler does not estimate again.
    ///
    /// Ordering by request alone could not tell the guard from a bare
    /// `call.send()`: alloy's gas filler asks for the estimate first there too,
    /// and the nonce filler asks while that estimate is still in flight. Only
    /// the answer marker separates the two.
    #[tokio::test]
    async fn a_passing_estimate_is_sent_with_the_nonce_reserved_after_it() {
        let rpc = ScriptedRpc::new(false);
        let nonces = PendingNonceManager::default();
        let (call, from) = scripted_call(rpc.clone(), nonces.clone());

        let pending = send_call_estimated(call, Network::Base)
            .await
            .expect("a call whose estimate passes is sent");
        assert_eq!(*pending.tx_hash(), FixedBytes::<32>::from([0xab; 32]));

        let calls = rpc.calls();
        let pos = |m: &str| {
            calls
                .iter()
                .position(|c| c == m)
                .unwrap_or_else(|| panic!("{m} never called: {calls:?}"))
        };
        assert!(
            pos(ESTIMATE_ANSWERED) < pos("eth_getTransactionCount"),
            "the nonce was requested before the estimate answered: {calls:?}"
        );
        assert!(
            pos("eth_getTransactionCount") < pos("eth_sendRawTransaction"),
            "{calls:?}"
        );
        assert_eq!(
            calls.iter().filter(|m| *m == "eth_estimateGas").count(),
            1,
            "the filler estimated a second time: {calls:?}"
        );
        assert_eq!(read_next(&nonces, from).await, Some(8));
    }

    // ---------------------------------------------------------------
    // EIP-1559 gas pricing.
    //
    // The numbers below are the real ones from the 2026-09-03 Polygon
    // incident, read out of the provider's txpool on 2026-09-10:
    // nonce 1157 was priced at maxFeePerGas 32.247 gwei / priority
    // 30.103 gwei against a base fee of 1.072 gwei, and the base fee
    // reached 248 gwei forty minutes later.
    // ---------------------------------------------------------------

    /// Base fee at block 93177231, the block the stuck transaction was priced on.
    const POLYGON_TROUGH_BASE_FEE: u128 = 1_072_065_664;
    /// Base fee once the trough recovered, and Polygon's steady state since.
    const POLYGON_STEADY_BASE_FEE: u128 = 248 * GWEI;
    /// What the node reported as the priority estimate in the trough.
    const POLYGON_TROUGH_PRIORITY: u128 = 30_103_229_849;

    #[test]
    fn polygon_priced_in_a_base_fee_trough_still_mines_after_the_recovery() {
        // This is the regression test for the incident. Alloy's estimator gave
        // 2 * 1.072 + 30.103 = 32.247 gwei here, which is what froze the signer.
        let floor = eip1559_fee_floor(Network::Polygon);
        let (priority, max_fee) =
            compute_eip1559_fees(POLYGON_TROUGH_BASE_FEE, POLYGON_TROUGH_PRIORITY, floor);

        assert!(
            max_fee > POLYGON_STEADY_BASE_FEE,
            "a settle priced in the trough ({max_fee} wei cap) would not survive \
             Polygon returning to its {POLYGON_STEADY_BASE_FEE} wei steady state \
             -- this is exactly how nonce 1157 wedged the signer for six days"
        );
        assert!(
            max_fee >= priority,
            "maxFeePerGas below maxPriorityFeePerGas is invalid"
        );
    }

    #[test]
    fn the_old_default_estimator_would_have_failed_that_same_case() {
        // Pins WHY the floor exists: the multiplier alone is not enough, so a
        // future simplification that drops `min_max_fee` fails here.
        let multiplier_only =
            POLYGON_TROUGH_BASE_FEE * BASE_FEE_MULTIPLIER + POLYGON_TROUGH_PRIORITY;
        assert!(
            multiplier_only < POLYGON_STEADY_BASE_FEE,
            "if the multiplier alone now clears the steady state, this test's \
             premise is stale -- re-measure before relaxing the floor"
        );
    }

    #[test]
    fn polygon_at_its_steady_state_is_priced_above_the_base_fee() {
        let floor = eip1559_fee_floor(Network::Polygon);
        let (_, max_fee) = compute_eip1559_fees(POLYGON_STEADY_BASE_FEE, 78 * GWEI, floor);
        assert!(max_fee > POLYGON_STEADY_BASE_FEE);
    }

    #[test]
    fn ethereum_keeps_the_floors_it_had_before_the_table_existed() {
        // The Ethereum branch this table replaced used exactly these numbers:
        // 1 gwei priority floor, 5 gwei cap floor, 2 gwei fallback base fee.
        let floor = eip1559_fee_floor(Network::Ethereum);
        assert_eq!(floor.min_priority, GWEI);
        assert_eq!(floor.min_max_fee, 5 * GWEI);
        assert_eq!(floor.fallback_base_fee, 2 * GWEI);

        // The 0.08 gwei auto-estimate that motivated the original branch.
        let (priority, max_fee) = compute_eip1559_fees(80_000_000, 0, floor);
        assert_eq!(priority, GWEI);
        assert_eq!(max_fee, 5 * GWEI);
    }

    #[test]
    fn an_unmeasured_chain_is_left_on_the_multiplier_alone() {
        // No floor invented for chains whose fee behaviour has not been
        // measured: they price exactly as alloy's estimator would, so this
        // change cannot move gas costs on a chain nobody looked at.
        let floor = eip1559_fee_floor(Network::Base);
        assert_eq!(floor.min_max_fee, 0);
        let (priority, max_fee) = compute_eip1559_fees(5_000_000, 2 * GWEI, floor);
        assert_eq!(priority, 2 * GWEI);
        assert_eq!(max_fee, 5_000_000 * BASE_FEE_MULTIPLIER + 2 * GWEI);
    }

    /// Base mainnet, 2026-09-14, measured: base fee 0.005 gwei, node priority
    /// estimate 0.001 gwei.
    const BASE_BASE_FEE: u128 = 5_000_000;
    const BASE_NODE_PRIORITY: u128 = 1_000_000;
    /// `gasUsed` of an EIP-3009 settle on Base that day (103,244 and 103,252).
    const BASE_SETTLE_GAS_USED: u128 = 103_244;
    /// The Base mainnet signer's balance once settles started failing, in wei.
    const BASE_DRAINED_BALANCE: u128 = 28_119_576_771_839;

    #[test]
    fn an_l2_settle_is_priced_on_the_nodes_tip_not_a_one_gwei_floor() {
        let floor = eip1559_fee_floor(Network::Base);
        let (priority, max_fee) = compute_eip1559_fees(BASE_BASE_FEE, BASE_NODE_PRIORITY, floor);
        assert_eq!(
            priority, BASE_NODE_PRIORITY,
            "a 1 gwei tip on a 0.005 gwei chain is 167x the price of the settle"
        );
        assert_eq!(
            max_fee,
            BASE_BASE_FEE * BASE_FEE_MULTIPLIER + BASE_NODE_PRIORITY
        );

        // What a settle pays is base fee + tip. `before` is the figure on the
        // 2026-09-14 receipts (0.00010376022 ETH); `after` is the same gas at
        // the node's tip.
        let before = BASE_SETTLE_GAS_USED * (BASE_BASE_FEE + GWEI);
        let after = BASE_SETTLE_GAS_USED * (BASE_BASE_FEE + priority);
        assert_eq!(before, 103_760_220_000_000);
        assert_eq!(after, 619_464_000_000);
        assert!(before / after >= 150);
    }

    #[test]
    fn the_drained_base_signer_can_settle_again_at_the_nodes_tip() {
        // The node reserves `gasLimit * maxFeePerGas` against the balance; the
        // send path sets the limit to the estimate times 5/4.
        let gas_limit = BASE_SETTLE_GAS_USED * 5 / 4;
        let floor = eip1559_fee_floor(Network::Base);
        let (_, max_fee) = compute_eip1559_fees(BASE_BASE_FEE, BASE_NODE_PRIORITY, floor);
        assert!(
            BASE_DRAINED_BALANCE / (gas_limit * max_fee) >= 10,
            "the balance that refused every settle at a 1.01 gwei cap admits \
             {} at {max_fee} wei",
            BASE_DRAINED_BALANCE / (gas_limit * max_fee)
        );
        assert_eq!(
            BASE_DRAINED_BALANCE / (gas_limit * (2 * BASE_BASE_FEE + GWEI)),
            0,
            "premise: at the old cap that balance admitted none"
        );
    }

    /// geth's default miner does not include a tip below 1 mwei.
    const ONE_MWEI: u128 = 1_000_000;

    #[test]
    fn arc_eurc_domains_match_independent_rpc_measurements() {
        // Public DOMAIN_SEPARATOR() values, measured on 2026-09-17.
        for (network, token, separator) in [
            (Network::Arc, address!("bEf5f6d51CB62b58e6A8f77868681825C6fe21c1"),
             "25fe3beaae16ef5c1cb9757c6efc1bf33f81ecd4c7dae191320372013b7d2175"),
            (Network::ArcTestnet, address!("89B50855Aa3bE2F677cD6303Cec089B5F319D72a"),
             "649ec6b0634bd74f28684781d2c9ae49dff14ba3d5f9bb5d70c1e1f0e1ebf160"),
        ] {
            let deployment = crate::network::EURCDeployment::by_network(network).unwrap();
            assert_eq!(deployment.decimals, 6);
            assert_eq!(deployment.address(), MixedAddress::from(token));
            let (name, version) = find_known_eip712_metadata(network, &token).unwrap();
            assert_eq!((name.as_str(), version.as_str()), ("EURC", "2"));
            let domain = eip712_domain! {
                name: name, version: version,
                chain_id: EvmChain::try_from(network).unwrap().chain_id,
                verifying_contract: token,
            };
            assert_eq!(hex::encode(domain.separator()), separator);
        }
    }

    /// hyperevm and arbitrum quote a zero tip outright, and when
    /// `eth_maxPriorityFeePerGas` fails the send path falls back to
    /// `floor.min_priority`. Neither may put a zero-tip transaction on the wire:
    /// a pool refuses it or a sequencer never mines it, and the nonce behind it
    /// waits with nothing to replace it.
    #[test]
    fn a_zero_or_missing_tip_estimate_still_tips_one_mwei() {
        for network in Network::variants() {
            let floor = eip1559_fee_floor(*network);
            // A node quoting zero, then the fallback a failed read takes.
            for node_tip in [0, floor.min_priority] {
                let (priority, max_fee) = compute_eip1559_fees(BASE_BASE_FEE, node_tip, floor);
                assert!(
                    priority >= ONE_MWEI,
                    "{network}: a {node_tip} wei estimate went out as a {priority} wei tip"
                );
                assert!(max_fee >= priority);
            }
        }
    }

    #[test]
    fn only_ethereum_and_polygon_tip_above_one_mwei() {
        for network in Network::variants() {
            let measured = matches!(
                network,
                Network::Ethereum
                    | Network::EthereumSepolia
                    | Network::Polygon
                    | Network::PolygonAmoy
            );
            let min_priority = eip1559_fee_floor(*network).min_priority;
            if measured {
                assert!(min_priority > ONE_MWEI, "{network} lost its measured floor");
            } else {
                assert_eq!(
                    min_priority, ONE_MWEI,
                    "{network}: a tip floor above 1 mwei must be measured for the chain it is on"
                );
            }
        }
    }

    #[test]
    fn max_fee_never_drops_below_priority() {
        // A node in a trough can report a priority far above 2 * baseFee. The
        // resulting transaction would be rejected outright as malformed.
        let floor = eip1559_fee_floor(Network::Base);
        let (priority, max_fee) = compute_eip1559_fees(1, 900 * GWEI, floor);
        assert!(max_fee >= priority);
    }

    #[test]
    fn absurd_inputs_saturate_instead_of_overflowing() {
        let floor = eip1559_fee_floor(Network::Polygon);
        let (priority, max_fee) = compute_eip1559_fees(u128::MAX, u128::MAX, floor);
        assert_eq!(priority, u128::MAX);
        assert_eq!(max_fee, u128::MAX);
    }

    #[test]
    fn test_is_nonce_error() {
        assert!(is_nonce_error(
            "nonce too low: next nonce 4604, tx nonce 4603"
        ));
        assert!(is_nonce_error(
            "ErrorResp(ErrorPayload { code: -32000, message: \"nonce too low\" })"
        ));
        assert!(is_nonce_error("transaction nonce already known"));
        assert!(is_nonce_error("replacement transaction underpriced"));
        assert!(is_nonce_error("Nonce gap: 16 > 15. Use nonce 15"));
        assert!(!is_nonce_error("insufficient funds for gas"));
        assert!(!is_nonce_error("execution reverted"));
        assert!(!is_nonce_error("Invalid signature"));
    }

    /// Phrasings the original `nonce && ...` conjunction let through untreated,
    /// so a recoverable collision surfaced to the caller as a hard settle
    /// failure instead of a resync + retry.
    #[test]
    fn test_is_nonce_error_short_phrasings() {
        // geth answers a duplicate submission without the word "nonce".
        assert!(is_nonce_error("already known"));
        assert!(is_nonce_error(
            "ErrorResp(ErrorPayload { code: -32000, message: \"already known\" })"
        ));
        // Several clients shorten the replacement error.
        assert!(is_nonce_error("replacement underpriced"));
        // A local nonce left ahead of the chain.
        assert!(is_nonce_error("nonce too high"));
        assert!(is_nonce_error("nonce has already been used"));

        // Still not nonce errors: retrying these would burn gas for nothing.
        assert!(!is_nonce_error("already mined"));
        assert!(!is_nonce_error("intrinsic gas too low"));
        assert!(!is_nonce_error("max fee per gas less than block base fee"));
    }

    // FAC-2 regression guard: the historical inverted-comparison bug rejected
    // spec-valid `validAfter=now-60` auths and accepted `validAfter`-in-the-future
    // ones. These lock the correct EIP-3009 ordering in place. `assert_time`
    // reads the real clock, so windows are anchored to `now` with wide margins.

    #[test]
    fn assert_time_accepts_spec_valid_window() {
        let now = UnixTimestamp::try_now().unwrap().0;
        let payer: MixedAddress = MixedAddress::Evm(EvmAddress(address!(
            "0000000000000000000000000000000000000001"
        )));
        assert!(
            assert_time(payer, UnixTimestamp(now - 60), UnixTimestamp(now + 3600)).is_ok(),
            "validAfter=now-60 / validBefore=now+3600 must be accepted"
        );
    }

    #[test]
    fn assert_time_rejects_future_valid_after() {
        let now = UnixTimestamp::try_now().unwrap().0;
        let payer: MixedAddress = MixedAddress::Evm(EvmAddress(address!(
            "0000000000000000000000000000000000000002"
        )));
        assert!(matches!(
            assert_time(payer, UnixTimestamp(now + 300), UnixTimestamp(now + 3600)),
            Err(FacilitatorLocalError::InvalidTiming(_, _))
        ));
    }

    #[test]
    fn assert_time_rejects_expired() {
        let now = UnixTimestamp::try_now().unwrap().0;
        let payer: MixedAddress = MixedAddress::Evm(EvmAddress(address!(
            "0000000000000000000000000000000000000003"
        )));
        assert!(matches!(
            assert_time(payer, UnixTimestamp(now - 3600), UnixTimestamp(now - 60)),
            Err(FacilitatorLocalError::InvalidTiming(_, _))
        ));
    }

    #[test]
    fn assert_time_rejects_within_expiry_grace_buffer() {
        // Deliberately conservative: an auth still nominally live but expiring
        // within the clock-skew/settlement grace buffer is rejected so a
        // settlement tx submitted now cannot expire on-chain before it mines.
        let now = UnixTimestamp::try_now().unwrap().0;
        let payer: MixedAddress = MixedAddress::Evm(EvmAddress(address!(
            "0000000000000000000000000000000000000004"
        )));
        assert!(matches!(
            assert_time(payer, UnixTimestamp(now - 60), UnixTimestamp(now + 2)),
            Err(FacilitatorLocalError::InvalidTiming(_, _))
        ));
    }

    // Test helpers that mirror the production get_next_nonce/reset_nonce
    // discipline: clone the per-address `Arc<Mutex<NonceState>>` out of the
    // dashmap and DROP the dashmap guard BEFORE `.lock().await`, so a shard
    // guard is never held across an await point (the guard-across-await hazard
    // behind the flaky CI test hang).
    async fn seed_state(
        m: &PendingNonceManager,
        addr: alloy::primitives::Address,
        next: Option<u64>,
        high_water: Option<u64>,
        last_allocated: Option<std::time::Instant>,
    ) {
        let lock = {
            let rm = m
                .nonces
                .entry(addr)
                .or_insert_with(|| Arc::new(Mutex::new(NonceState::default())));
            Arc::clone(rm.value())
        };
        let mut state = lock.lock().await;
        state.next = next;
        state.high_water = high_water;
        state.last_allocated = last_allocated;
    }

    async fn read_next(m: &PendingNonceManager, addr: alloy::primitives::Address) -> Option<u64> {
        let lock = m.nonces.get(&addr).map(|r| Arc::clone(r.value()));
        match lock {
            Some(l) => l.lock().await.next,
            None => None,
        }
    }

    async fn read_high_water(
        m: &PendingNonceManager,
        addr: alloy::primitives::Address,
    ) -> Option<u64> {
        let lock = m.nonces.get(&addr).map(|r| Arc::clone(r.value()));
        match lock {
            Some(l) => l.lock().await.high_water,
            None => None,
        }
    }

    /// Exercises the resync decision without a live provider. Delegates to
    /// [`resync_target`] rather than re-deriving the branch — see that
    /// function's doc comment for why a second copy is a trap, not a
    /// convenience.
    fn resync_nonce(
        pending: u64,
        high_water: Option<u64>,
        last_allocated: Option<std::time::Instant>,
    ) -> u64 {
        resync_target(pending, high_water, last_allocated, None)
    }

    // ---------------------------------------------------------------
    // Downward resync on sustained drift.
    //
    // The shape measured on 2026-09-10: a Polygon signer wedged behind an
    // unmineable transaction, the node refusing every new send before it
    // reached the mempool, and this process's high-water mark ratcheting to
    // 1738 while the chain reported 1557.
    // ---------------------------------------------------------------

    const DRIFTED: u64 = 1557; // chain pending
    const RATCHETED: u64 = 1737; // local high-water mark

    #[test]
    fn drift_below_the_threshold_still_protects_in_flight_transactions() {
        // Four minutes of divergence is well inside normal propagation trouble.
        // Rewinding here would try to REPLACE a transaction that is merely slow.
        let target = resync_target(
            DRIFTED,
            Some(RATCHETED),
            Some(std::time::Instant::now()),
            Some(std::time::Duration::from_secs(240)),
        );
        assert_eq!(target, RATCHETED + 1);
    }

    #[test]
    fn sustained_drift_gives_up_the_high_water_mark() {
        // Five minutes of the chain reporting less than we believe. Nothing we
        // allocated is still propagating; keeping the mark would leave
        // 1557..1737 permanently empty once sends are accepted again.
        let target = resync_target(
            DRIFTED,
            Some(RATCHETED),
            Some(std::time::Instant::now()),
            Some(std::time::Duration::from_secs(300)),
        );
        assert_eq!(
            target, DRIFTED,
            "high-water mark survived sustained drift; recovery would open a real nonce gap"
        );
    }

    #[test]
    fn sustained_drift_does_not_rewind_when_the_chain_is_ahead() {
        // The chain has seen everything we allocated and more. There is no
        // drift to resolve, and `pending` is the answer either way -- but it
        // must come from the ordinary branch, not from the escape hatch.
        let target = resync_target(
            RATCHETED + 50,
            Some(RATCHETED),
            Some(std::time::Instant::now()),
            Some(std::time::Duration::from_secs(3_600)),
        );
        assert_eq!(target, RATCHETED + 50);
    }

    #[test]
    fn the_quiet_period_branch_still_wins_when_both_apply() {
        // Both escapes agree on the answer; this pins that adding the second
        // one did not change what the first one does.
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(600);
        assert_eq!(
            resync_target(DRIFTED, Some(RATCHETED), Some(long_ago), None),
            DRIFTED
        );
        assert_eq!(
            resync_target(
                DRIFTED,
                Some(RATCHETED),
                Some(long_ago),
                Some(std::time::Duration::from_secs(600))
            ),
            DRIFTED
        );
    }

    #[test]
    fn drift_alone_cannot_rewind_an_address_with_no_high_water_mark() {
        // Nothing was ever allocated here, so there is no mark to abandon and
        // no in-flight transaction to protect.
        assert_eq!(
            resync_target(
                DRIFTED,
                None,
                None,
                Some(std::time::Duration::from_secs(3_600))
            ),
            DRIFTED
        );
    }

    #[tokio::test]
    async fn test_reset_nonce_forces_resync() {
        let manager = PendingNonceManager::default();
        let test_address = address!("0000000000000000000000000000000000000001");
        seed_state(&manager, test_address, Some(42), Some(41), None).await;
        assert_eq!(read_next(&manager, test_address).await, Some(42));

        manager.reset_nonce(test_address).await;

        // The next allocation must go back to the chain...
        assert_eq!(read_next(&manager, test_address).await, None);
        // ...but the high-water mark survives, so the resync cannot rewind
        // underneath a transaction that is still in flight.
        assert_eq!(read_high_water(&manager, test_address).await, Some(41));
    }

    #[tokio::test]
    async fn test_reset_nonce_preserves_high_water_after_allocations() {
        let manager = PendingNonceManager::default();
        let test_address = address!("0000000000000000000000000000000000000002");

        // Three concurrent settles allocated 50, 51 and 52; then one failed.
        seed_state(&manager, test_address, Some(53), Some(52), None).await;
        manager.reset_nonce(test_address).await;

        assert_eq!(read_next(&manager, test_address).await, None);
        assert_eq!(read_high_water(&manager, test_address).await, Some(52));
    }

    /// `nonce too high` is the one nonce error that says the CHAIN is behind
    /// us, so it must be told apart from the ones that say the opposite.
    #[test]
    fn only_nonce_too_high_proves_the_chain_is_behind() {
        // The exact phrasing observed in production on 2026-09-01.
        assert!(is_nonce_too_high(
            "nonce too high: address 0x1030..13C7, tx: 1603 state: 1555"
        ));
        assert!(is_nonce_too_high("Nonce Too High"));

        // These mean the chain is at or PAST our nonce: something of ours
        // landed, so the high-water mark still protects a real sibling.
        assert!(!is_nonce_too_high(
            "nonce too low: next nonce 4604, tx nonce 4603"
        ));
        assert!(!is_nonce_too_high("replacement transaction underpriced"));
        assert!(!is_nonce_too_high("already known"));
        assert!(!is_nonce_too_high("nonce has already been used"));

        // And it must still be a retryable nonce error, or the retry loop
        // would never reach the recovery.
        assert!(is_nonce_error(
            "nonce too high: address 0x1030..13C7, tx: 1603 state: 1555"
        ));
    }

    /// THE regression. `resync_to_chain` must drop the high-water mark, or the
    /// ratchet returns.
    ///
    /// Production, 2026-09-01, minutes after a restart had cleared the state:
    /// the first failure opened a gap of 1 (tx 1556 against chain state 1555),
    /// one second later it was 31, and 65 seconds later 48. `reset_nonce`
    /// preserves the mark and leans on the 120-second idle window to release
    /// it, but a sustained failure burst refreshes `last_allocated` on every
    /// attempt, so that window never elapses.
    #[tokio::test]
    async fn resync_to_chain_discards_the_mark_that_pins_the_ratchet() {
        let manager = PendingNonceManager::default();
        let test_address = address!("0000000000000000000000000000000000000003");

        // The measured state: the chain is at 1555, we have climbed to 1603.
        seed_state(&manager, test_address, Some(1604), Some(1603), None).await;
        manager.resync_to_chain(test_address).await;

        assert_eq!(read_next(&manager, test_address).await, None);
        assert_eq!(
            read_high_water(&manager, test_address).await,
            None,
            "the high-water mark must be discarded, or the next resync returns \
             high_water + 1 and the ratchet climbs again"
        );

        // With no mark, a resync takes the chain's own count -- which is the
        // whole point.
        assert_eq!(resync_target(1555, None, None, None), 1555);
    }

    /// `reset_nonce` must KEEP preserving the mark. The two recoveries answer
    /// different evidence and collapsing them would undo the protection against
    /// rewinding underneath an in-flight sibling.
    #[tokio::test]
    async fn reset_and_resync_stay_different_recoveries() {
        let manager = PendingNonceManager::default();
        let addr = address!("0000000000000000000000000000000000000004");

        seed_state(&manager, addr, Some(53), Some(52), None).await;
        manager.reset_nonce(addr).await;
        assert_eq!(
            read_high_water(&manager, addr).await,
            Some(52),
            "reset_nonce must still preserve the mark"
        );

        manager.resync_to_chain(addr).await;
        assert_eq!(read_high_water(&manager, addr).await, None);
    }

    /// The ratchet itself, spelled out: while a mark survives, every resync
    /// hands back one PAST it, so each failure climbs.
    #[test]
    fn a_surviving_mark_is_what_makes_the_ratchet_climb() {
        // Chain stuck at 1555; the mark keeps climbing with each failed try.
        assert_eq!(resync_target(1555, Some(1555), None, None), 1556);
        assert_eq!(resync_target(1555, Some(1586), None, None), 1587);
        assert_eq!(resync_target(1555, Some(1602), None, None), 1603);
        // Drop the mark and it collapses back to the chain in one step.
        assert_eq!(resync_target(1555, None, None, None), 1555);
    }

    /// A provider pointed at a closed port.
    ///
    /// Used where the allocation must be served from cached state: if a refactor
    /// ever makes that path touch the network, these tests fail loudly instead
    /// of passing while quietly adding an RPC round-trip to every settle.
    fn offline_provider() -> impl Provider<AlloyEthereum> {
        ProviderBuilder::default().connect_client(RpcClient::new_http(
            "http://127.0.0.1:1".parse().expect("static url"),
        ))
    }

    /// **The success criterion from the concurrency handoff, actually executed.**
    ///
    /// It had been carried as "20 concurrent settles, zero `nonce too low`" and
    /// verified by nobody — the nonce work was covered by unit tests that each
    /// allocate once, which cannot observe the failure this guards against.
    ///
    /// A duplicate nonce IS `nonce too low`: two transactions signed with the
    /// same number, the second rejected by the node. So the property to assert
    /// is not "no error" but "no repeats" — 20 concurrent allocations must yield
    /// 20 distinct, contiguous nonces.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn test_concurrent_allocations_never_hand_out_the_same_nonce() {
        const CONCURRENT: u64 = 20;
        const START: u64 = 100;

        let manager = Arc::new(PendingNonceManager::default());
        let addr = address!("0000000000000000000000000000000000000020");
        seed_state(&manager, addr, Some(START), Some(START - 1), None).await;

        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..CONCURRENT {
            let manager = Arc::clone(&manager);
            tasks.spawn(async move {
                let provider = offline_provider();
                manager
                    .get_next_nonce(&provider, addr)
                    .await
                    .expect("cached allocation must not need the network")
            });
        }

        let mut allocated = Vec::new();
        while let Some(result) = tasks.join_next().await {
            allocated.push(result.expect("allocation task panicked"));
        }
        allocated.sort_unstable();

        let unique: std::collections::HashSet<_> = allocated.iter().copied().collect();
        assert_eq!(
            unique.len(),
            CONCURRENT as usize,
            "duplicate nonce handed out under concurrency: {allocated:?}"
        );
        // Contiguous, not merely distinct: a gap strands every later settle
        // behind it until the chain-trust window expires.
        let expected: Vec<u64> = (START..START + CONCURRENT).collect();
        assert_eq!(
            allocated, expected,
            "nonces must be contiguous from {START}"
        );

        assert_eq!(read_next(&manager, addr).await, Some(START + CONCURRENT));
        assert_eq!(
            read_high_water(&manager, addr).await,
            Some(START + CONCURRENT - 1)
        );
    }

    /// The same race across several signers, which is what a pool actually
    /// changes. Per-address state must stay independent: one busy signer must
    /// not shift another's sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn test_concurrent_allocations_stay_independent_per_signer() {
        const PER_SIGNER: u64 = 10;
        let manager = Arc::new(PendingNonceManager::default());
        let signers = [
            address!("0000000000000000000000000000000000000021"),
            address!("0000000000000000000000000000000000000022"),
            address!("0000000000000000000000000000000000000023"),
        ];
        for (i, addr) in signers.iter().enumerate() {
            seed_state(&manager, *addr, Some(i as u64 * 1000), None, None).await;
        }

        let mut tasks = tokio::task::JoinSet::new();
        for (i, addr) in signers.iter().enumerate() {
            for _ in 0..PER_SIGNER {
                let manager = Arc::clone(&manager);
                let addr = *addr;
                tasks.spawn(async move {
                    let provider = offline_provider();
                    (
                        i,
                        manager
                            .get_next_nonce(&provider, addr)
                            .await
                            .expect("alloc"),
                    )
                });
            }
        }

        let mut by_signer: std::collections::HashMap<usize, Vec<u64>> =
            std::collections::HashMap::new();
        while let Some(result) = tasks.join_next().await {
            let (i, nonce) = result.expect("allocation task panicked");
            by_signer.entry(i).or_default().push(nonce);
        }

        for (i, mut nonces) in by_signer {
            nonces.sort_unstable();
            let base = i as u64 * 1000;
            let expected: Vec<u64> = (base..base + PER_SIGNER).collect();
            assert_eq!(nonces, expected, "signer {i} sequence was disturbed");
        }
    }

    /// The regression that motivated the high-water mark: a resync that lands on
    /// a node lagging behind the mempool reports a nonce we already handed out.
    /// Reusing it would replace an in-flight settle rather than queue behind it.
    #[test]
    fn test_resync_never_rewinds_below_in_flight_high_water() {
        let just_now = Some(std::time::Instant::now());
        assert_eq!(resync_nonce(50, Some(52), just_now), 53);
        assert_eq!(resync_nonce(52, Some(52), just_now), 53);
    }

    /// A resync that is genuinely ahead of us is trusted as-is.
    #[test]
    fn test_resync_accepts_chain_when_ahead() {
        let just_now = Some(std::time::Instant::now());
        assert_eq!(resync_nonce(60, Some(52), just_now), 60);
        assert_eq!(resync_nonce(7, None, None), 7);
    }

    /// Once nothing we allocated can still be in flight, the chain wins even if
    /// that rewinds — otherwise a dropped transaction would leave the signer
    /// stuck behind a nonce gap forever.
    #[test]
    fn test_resync_trusts_chain_after_quiet_period() {
        let long_ago = Some(std::time::Instant::now() - NONCE_TRUST_CHAIN_AFTER);
        assert_eq!(resync_nonce(50, Some(52), long_ago), 50);
    }

    /// The self-DoS guard. Alloy fills gas and nonce concurrently, so a
    /// reverting payload consumes a nonce for a transaction that never reaches
    /// the network. Handing it back keeps the signer usable; leaving it would
    /// stall every later settle behind a gap for NONCE_TRUST_CHAIN_AFTER.
    #[tokio::test]
    async fn test_release_nonce_rolls_back_unbroadcast_allocation() {
        let manager = PendingNonceManager::default();
        let addr = address!("0000000000000000000000000000000000000010");

        // Allocated nonce 7; nothing followed it.
        seed_state(&manager, addr, Some(8), Some(7), None).await;
        manager.release_nonce(addr, 7).await;

        assert_eq!(read_next(&manager, addr).await, Some(7));
        assert_eq!(read_high_water(&manager, addr).await, Some(6));
    }

    /// If a sibling settle already took the next nonce, rolling back would
    /// hand out a nonce that is genuinely in flight. The gap is real and the
    /// resync path owns healing it.
    #[tokio::test]
    async fn test_release_nonce_declines_when_another_allocation_followed() {
        let manager = PendingNonceManager::default();
        let addr = address!("0000000000000000000000000000000000000011");

        // We took 7, then a sibling took 8.
        seed_state(&manager, addr, Some(9), Some(8), None).await;
        manager.release_nonce(addr, 7).await;

        assert_eq!(read_next(&manager, addr).await, Some(9));
        assert_eq!(read_high_water(&manager, addr).await, Some(8));
    }

    #[tokio::test]
    async fn test_release_nonce_zero_clears_high_water() {
        let manager = PendingNonceManager::default();
        let addr = address!("0000000000000000000000000000000000000012");
        seed_state(&manager, addr, Some(1), Some(0), None).await;
        manager.release_nonce(addr, 0).await;
        assert_eq!(read_next(&manager, addr).await, Some(0));
        assert_eq!(read_high_water(&manager, addr).await, None);
    }

    /// Only failures that provably never entered the mempool may return their
    /// nonce. Anything ambiguous keeps the conservative reset.
    #[test]
    fn test_pre_broadcast_rejection_classification() {
        assert!(is_pre_broadcast_rejection("execution reverted"));
        assert!(is_pre_broadcast_rejection("gas required exceeds allowance"));
        assert!(is_pre_broadcast_rejection("intrinsic gas too low"));
        assert!(is_pre_broadcast_rejection(
            "insufficient funds for transfer"
        ));
        assert!(is_pre_broadcast_rejection(
            "max fee per gas less than block base fee"
        ));

        // Nonce errors resync rather than release: our counter is what is wrong.
        assert!(!is_pre_broadcast_rejection("nonce too low"));
        assert!(!is_pre_broadcast_rejection("already known"));
        assert!(!is_pre_broadcast_rejection("replacement underpriced"));
        // Ambiguous: the transaction may be propagating.
        assert!(!is_pre_broadcast_rejection("operation timed out"));
        assert!(!is_pre_broadcast_rejection("error sending request for url"));
        assert!(!is_pre_broadcast_rejection(""));

        // txpool is full: the transaction never entered the mempool, so the
        // nonce provably was not consumed. Real geth phrasing (2026-08-20
        // incident): the code alone (-32003) is NOT enough to classify this,
        // see `is_mempool_full`'s doc comment and the negative case below.
        assert!(is_pre_broadcast_rejection(
            r#"ErrorResp(ErrorPayload { code: -32003, message: "txpool is full" })"#
        ));
    }

    /// `-32003` is overloaded: it also carries `eth_call`'s out-of-gas
    /// rejection (see `handlers.rs`'s `OWNER_SCAN_BATCH` doc comment), which
    /// IS a real answer from the chain, not evidence the tx never entered the
    /// mempool. Matching on the code instead of the message would release a
    /// nonce that may already be in flight.
    #[test]
    fn test_is_mempool_full_matches_message_not_code() {
        assert!(is_mempool_full(
            r#"ErrorResp(ErrorPayload { code: -32003, message: "txpool is full" })"#
        ));
        assert!(is_mempool_full(
            "txpool is full: already have 4096 pending transactions in queue"
        ));

        assert!(!is_mempool_full(
            "server returned an error response: error code -32003: out of gas: \
             gas exhausted during memory expansion: 600000000"
        ));
        assert!(!is_mempool_full("execution reverted"));
        assert!(!is_mempool_full(""));
    }

    #[tokio::test]
    async fn test_reset_nonce_on_nonexistent_address() {
        let manager = PendingNonceManager::default();
        let test_address = address!("0000000000000000000000000000000000000099");

        // Reset should not panic on address that hasn't been used
        manager.reset_nonce(test_address).await;

        // Verify nonce map still doesn't have this address
        assert!(!manager.nonces.contains_key(&test_address));
    }

    #[tokio::test]
    async fn test_multiple_addresses_independent_nonces() {
        let manager = PendingNonceManager::default();
        let address1 = address!("0000000000000000000000000000000000000001");
        let address2 = address!("0000000000000000000000000000000000000002");

        seed_state(&manager, address1, Some(10), Some(9), None).await;
        seed_state(&manager, address2, Some(20), Some(19), None).await;

        manager.reset_nonce(address1).await;

        // address1 is reset; address2 is untouched. Signers in a pool must not
        // share a nonce lane.
        assert_eq!(read_next(&manager, address1).await, None);
        assert_eq!(read_next(&manager, address2).await, Some(20));
    }

    #[tokio::test]
    async fn test_concurrent_reset_and_access() {
        let manager = Arc::new(PendingNonceManager::default());
        let test_address = address!("0000000000000000000000000000000000000003");

        seed_state(&manager, test_address, Some(100), Some(99), None).await;

        // Spawn concurrent tasks
        let manager1 = Arc::clone(&manager);
        let handle1 = tokio::spawn(async move {
            manager1.reset_nonce(test_address).await;
        });

        let manager2 = Arc::clone(&manager);
        let handle2 = tokio::spawn(async move {
            manager2.reset_nonce(test_address).await;
        });

        // Wait for both to complete
        handle1.await.unwrap();
        handle2.await.unwrap();

        assert_eq!(read_next(&manager, test_address).await, None);
        assert_eq!(read_high_water(&manager, test_address).await, Some(99));
    }
}

/// A settlement that is broadcast and never confirmed must hand the caller the
/// transaction hash.
///
/// The receipt wait is the ONE place where "settlement submitted" and
/// "settlement confirmed" are different states after the response has gone out.
/// Until now that branch answered `ContractCall`, which reaches the client as
/// `contract_call_failed (ref: <uuid>)` -- a correlation id only this
/// facilitator can resolve, for a transaction that may be sitting mined on
/// chain. With nothing to look up, the only move left to the caller is the
/// retry the branch exists to prevent.
///
/// The mock RPC accepts the transaction and then answers `null` to every
/// receipt read, so the timeout in `send_transaction_from` is the real one, not
/// a stubbed error. `elapsed >= timeout` is asserted for exactly that reason: a
/// transport error would fail this branch instantly and take the same code
/// path, which would make the test pass without ever exercising a timeout.
#[cfg(test)]
mod settlement_unconfirmed_tests {
    use super::*;
    use alloy::network::EthereumWallet;
    use alloy::signers::local::PrivateKeySigner;
    use axum::{routing::post, Json as AxumJson, Router};
    use serde_json::{json, Value};

    /// The hash the mock hands back for `eth_sendRawTransaction`.
    const SUBMITTED_TX: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

    /// Answers the JSON-RPC calls one `send_transaction_from` makes, and never
    /// produces a receipt.
    fn answer(method: &str) -> Value {
        match method {
            // Base's chain id: the provider is built for `Network::Base` below.
            "eth_chainId" => json!("0x2105"),
            "eth_getTransactionCount" => json!("0x0"),
            "eth_gasPrice" => json!("0x3b9aca00"),
            "eth_maxPriorityFeePerGas" => json!("0x3b9aca00"),
            "eth_estimateGas" => json!("0x5208"),
            "eth_sendRawTransaction" => json!(SUBMITTED_TX),
            // The block height never moves, so the heartbeat never has a block
            // to check the transaction against and the wait runs to its end.
            "eth_blockNumber" => json!("0x1"),
            // The point of the whole fixture: mined or not, we never find out.
            "eth_getTransactionReceipt" => Value::Null,
            _ => Value::Null,
        }
    }

    /// Every `eth_sendRawTransaction` any mock of this module has answered.
    static RAW_SENDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    async fn rpc(AxumJson(body): AxumJson<Value>) -> AxumJson<Value> {
        let one = |req: &Value| {
            if req.get("method").and_then(Value::as_str) == Some("eth_sendRawTransaction") {
                RAW_SENDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            json!({
                "jsonrpc": "2.0",
                "id": req.get("id").cloned().unwrap_or(json!(1)),
                "result": answer(req.get("method").and_then(Value::as_str).unwrap_or("")),
            })
        };
        AxumJson(match &body {
            Value::Array(reqs) => Value::Array(reqs.iter().map(one).collect()),
            req => one(req),
        })
    }

    async fn spawn_rpc() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", post(rpc)))
                .await
                .unwrap();
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn a_settle_that_never_confirms_returns_the_transaction_hash() {
        let url = spawn_rpc().await;
        // 1s so the test costs a second rather than Base's 90.
        std::env::set_var("TX_RECEIPT_TIMEOUT_SECS", "1");

        let signer = PrivateKeySigner::random();
        // `eip1559 = false` so gas pricing is one `eth_gasPrice` call rather
        // than a fee-history negotiation: the fixture is about the receipt,
        // not about pricing.
        let provider =
            EvmProvider::try_new(EthereumWallet::from(signer), &url, false, Network::Base)
                .await
                .expect("provider");

        let started = std::time::Instant::now();
        let result = provider
            .send_transaction(MetaTransaction {
                authorization_list: None,
                to: address!("0000000000000000000000000000000000000001"),
                calldata: Bytes::from_static(&[0u8; 4]),
                confirmations: 1,
            })
            .await;
        let elapsed = started.elapsed();

        std::env::remove_var("TX_RECEIPT_TIMEOUT_SECS");

        match result {
            Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                assert_eq!(
                    tx.to_string(),
                    SUBMITTED_TX,
                    "the error carries a different transaction than the one we broadcast",
                );
                assert_eq!(network, Network::Base);
            }
            other => panic!(
                "a broadcast transaction whose receipt never arrived must report \
                 SettlementUnconfirmed with its hash, so the caller has something to look up \
                 on chain; got {other:?}",
            ),
        }

        assert!(
            elapsed >= std::time::Duration::from_secs(1),
            "returned in {elapsed:?}, faster than the 1s receipt wait -- this failed on \
             transport before the timeout ever ran, so the fixture is not exercising the \
             timeout path it claims to",
        );
    }

    fn meta_transaction() -> MetaTransaction {
        MetaTransaction {
            authorization_list: None,
            to: address!("0000000000000000000000000000000000000001"),
            calldata: Bytes::from_static(&[0u8; 4]),
            confirmations: 1,
        }
    }

    /// Under a receipt admission the send path says, through the real hooks,
    /// whether anything left: a lost writer lease sends nothing and latches
    /// nothing, so the admission can be released; a send stores its bytes
    /// first and latches, after which no failure releases it.
    #[tokio::test]
    async fn under_a_receipt_admission_only_a_send_that_never_started_can_be_released() {
        use std::sync::atomic::Ordering;
        let url = spawn_rpc().await;
        std::env::set_var("TX_RECEIPT_TIMEOUT_SECS", "1");
        let provider = EvmProvider::try_new(
            EthereumWallet::from(PrivateKeySigner::random()),
            &url,
            false,
            Network::Base,
        )
        .await
        .expect("provider");

        let sends_before = RAW_SENDS.load(Ordering::SeqCst);
        crate::writer_lease::set_writer_for_test(false);
        let lost = crate::receipts::with_test_admission(async {
            provider.send_transaction(meta_transaction()).await
        })
        .await;
        crate::writer_lease::set_writer_for_test(true);
        assert!(
            matches!(
                lost.output,
                Err(FacilitatorLocalError::WriterLeaseUnavailable(_))
            ),
            "{:?}",
            lost.output
        );
        assert_eq!(RAW_SENDS.load(Ordering::SeqCst), sends_before);
        assert!(!lost.latched);
        assert!(lost.prepared.is_none());

        let sent = crate::receipts::with_test_admission(async {
            let result = provider.send_transaction(meta_transaction()).await;
            // What the dispatch does with any error: after the latch, ignored.
            crate::receipts::unsent("evm_settle");
            result
        })
        .await;
        std::env::remove_var("TX_RECEIPT_TIMEOUT_SECS");
        assert!(
            matches!(
                sent.output,
                Err(FacilitatorLocalError::SettlementUnconfirmed(..))
            ),
            "{:?}",
            sent.output
        );
        assert_eq!(RAW_SENDS.load(Ordering::SeqCst), sends_before + 1);
        assert!(sent.latched);
        assert!(sent.prepared.is_some(), "bytes are stored before the send");
        assert_eq!(sent.unsent, None, "a mark after the latch released a send");
    }

    /// The production wiring: `NetworkProvider` marks any EVM settle error.
    #[tokio::test]
    async fn an_evm_settle_error_before_the_send_is_marked_unsent() {
        use crate::facilitator::Facilitator as _;
        let url = spawn_rpc().await;
        let provider = EvmProvider::try_new(
            EthereumWallet::from(PrivateKeySigner::random()),
            &url,
            false,
            Network::Base,
        )
        .await
        .expect("provider");
        let request: SettleRequest = serde_json::from_value(json!({
            "x402Version":1,
            "paymentPayload":{"x402Version":1,"scheme":"exact","network":"base","payload":{
                "signature":format!("0x{}", "11".repeat(65)),"authorization":{
                    "from":"0x1111111111111111111111111111111111111111",
                    "to":"0x2222222222222222222222222222222222222222",
                    "value":"1000","validAfter":"0","validBefore":"2000000000",
                    "nonce":format!("0x{}", "01".repeat(32))}}},
            "paymentRequirements":{"scheme":"exact","network":"base","maxAmountRequired":"1000",
                "resource":"https://merchant.example/data","description":"fixture",
                "mimeType":"application/json","payTo":"0x2222222222222222222222222222222222222222",
                "maxTimeoutSeconds":60,"asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "extra":{"name":"USD Coin","version":"2"}}
        }))
        .unwrap();
        let settled = crate::receipts::with_test_admission(async {
            crate::chain::NetworkProvider::Evm(provider)
                .settle(&request)
                .await
        })
        .await;
        assert!(settled.output.is_err());
        assert_eq!(settled.unsent, Some("evm_settle"));
        assert!(!settled.latched);
    }
}

/// A send that fails AFTER the bytes reached the transport.
///
/// `settlement_unconfirmed_tests` covers a send the node accepted and never
/// mined; this covers the node's answer to the send itself going missing or
/// saying it already holds the transaction. Either can end mined, so the error
/// is `SettlementUnconfirmed` carrying the hash of the exact bytes sent, and
/// nothing is sent again. A node that answered and refused keeps the old
/// handling: that transaction never queued.
#[cfg(test)]
mod broadcast_outcome_tests {
    use super::*;
    use alloy::network::EthereumWallet;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::transports::{RpcError, TransportError, TransportErrorKind};
    use axum::{extract::State, response::IntoResponse, routing::post, Json as AxumJson, Router};
    use serde_json::{json, Value};
    use std::sync::Mutex as StdMutex;

    /// A JSON-RPC error the node answered with. Built through `deser_err`,
    /// which is how alloy turns an error payload into `ErrorResp`.
    fn node_said(message: &str) -> TransportError {
        let payload = json!({"code": -32000, "message": message}).to_string();
        let error =
            TransportError::deser_err(serde_json::from_str::<u8>("x").unwrap_err(), payload);
        assert!(matches!(error, RpcError::ErrorResp(_)), "{error:?}");
        error
    }

    #[test]
    fn only_a_node_verdict_proves_the_transaction_never_queued() {
        let lost = serde_json::from_str::<u8>("x").unwrap_err();
        let cases: Vec<(&str, TransportError, bool)> = vec![
            ("already known", node_said("already known"), true),
            ("nonce too low", node_said("nonce too low"), false),
            (
                "gas shortfall",
                node_said("insufficient funds for gas * price + value"),
                false,
            ),
            ("txpool full", node_said("txpool is full"), false),
            (
                "gateway 502",
                TransportErrorKind::http_error(502, "bad gateway".into()),
                true,
            ),
            (
                "gateway 504",
                TransportErrorKind::http_error(504, String::new()),
                true,
            ),
            (
                "rate limit 429",
                TransportErrorKind::http_error(429, "Too Many Requests".into()),
                false,
            ),
            (
                "verdict inside a 500",
                TransportErrorKind::http_error(
                    500,
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"nonce too low"}}"#
                        .into(),
                ),
                false,
            ),
            (
                "already known inside a 500",
                TransportErrorKind::http_error(
                    500,
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"already known"}}"#
                        .into(),
                ),
                true,
            ),
            (
                "unreadable answer",
                TransportError::deser_err(lost, "<html>upstream reset</html>"),
                true,
            ),
            ("null answer", RpcError::NullResp, true),
            (
                "retries exhausted",
                TransportErrorKind::custom_str("Max retries exceeded HTTP error 503"),
                true,
            ),
            (
                "never serialized",
                RpcError::SerError(serde_json::from_str::<u8>("x").unwrap_err()),
                false,
            ),
        ];
        for (name, error, queued) in cases {
            assert_eq!(
                broadcast_may_have_queued(&error),
                queued,
                "{name}: {error:?}"
            );
        }
    }

    /// What the mock node does with `eth_sendRawTransaction`.
    #[derive(Clone)]
    enum SendRaw {
        HttpStatus(u16),
        NodeError(&'static str),
    }

    #[derive(Clone)]
    struct Node {
        send_raw: SendRaw,
        sent: Arc<StdMutex<Vec<String>>>,
    }

    fn result(req: &Value, value: Value) -> Value {
        json!({"jsonrpc":"2.0","id":req.get("id").cloned().unwrap_or(json!(1)),"result":value})
    }

    async fn rpc(
        State(node): State<Node>,
        AxumJson(body): AxumJson<Value>,
    ) -> axum::response::Response {
        let reqs = match &body {
            Value::Array(reqs) => reqs.clone(),
            req => vec![req.clone()],
        };
        let mut answers = Vec::new();
        for req in &reqs {
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");
            let answer = match method {
                "eth_chainId" => result(req, json!("0x2105")),
                "eth_getTransactionCount" => result(req, json!("0x0")),
                "eth_gasPrice" | "eth_maxPriorityFeePerGas" => result(req, json!("0x3b9aca00")),
                "eth_estimateGas" => result(req, json!("0x5208")),
                "eth_sendRawTransaction" => {
                    let raw = req["params"][0].as_str().unwrap_or_default().to_string();
                    node.sent.lock().unwrap().push(raw);
                    match node.send_raw {
                        SendRaw::HttpStatus(code) => {
                            return (
                                axum::http::StatusCode::from_u16(code).unwrap(),
                                "upstream connection reset",
                            )
                                .into_response()
                        }
                        SendRaw::NodeError(message) => json!({"jsonrpc":"2.0",
                            "id":req.get("id").cloned().unwrap_or(json!(1)),
                            "error":{"code":-32000,"message":message}}),
                    }
                }
                _ => result(req, Value::Null),
            };
            answers.push(answer);
        }
        AxumJson(if body.is_array() {
            Value::Array(answers)
        } else {
            answers.remove(0)
        })
        .into_response()
    }

    async fn provider_against(send_raw: SendRaw) -> (EvmProvider, Arc<StdMutex<Vec<String>>>) {
        let sent = Arc::new(StdMutex::new(Vec::new()));
        let node = Node {
            send_raw,
            sent: sent.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let app = Router::new().route("/", post(rpc)).with_state(node);
            axum::serve(listener, app).await.unwrap();
        });
        let provider = EvmProvider::try_new(
            EthereumWallet::from(PrivateKeySigner::random()),
            &format!("http://{addr}/"),
            false,
            Network::Base,
        )
        .await
        .expect("provider");
        (provider, sent)
    }

    fn meta_transaction() -> MetaTransaction {
        MetaTransaction {
            authorization_list: None,
            to: address!("0000000000000000000000000000000000000001"),
            calldata: Bytes::from_static(&[0u8; 4]),
            confirmations: 1,
        }
    }

    /// The hash of the raw transaction the node was handed, as its explorer
    /// would print it.
    fn hash_of(raw: &str) -> String {
        let bytes = hex::decode(raw.trim_start_matches("0x")).expect("raw tx is hex");
        alloy::primitives::keccak256(bytes).to_string()
    }

    #[tokio::test]
    async fn a_send_whose_answer_is_lost_reports_the_hash_it_sent_and_sends_once() {
        for send_raw in [
            SendRaw::HttpStatus(502),
            SendRaw::NodeError("already known"),
        ] {
            let (provider, sent) = provider_against(send_raw).await;
            let result = provider.send_transaction(meta_transaction()).await;
            let sent = sent.lock().unwrap().clone();
            assert_eq!(
                sent.len(),
                1,
                "sent again after a send that may have queued"
            );
            match result {
                Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                    assert_eq!(tx.to_string(), hash_of(&sent[0]));
                    assert_eq!(network, Network::Base);
                }
                other => panic!("expected SettlementUnconfirmed with the sent hash, got {other:?}"),
            }
        }
    }

    /// A node that refused keeps today's answer: it never queued the
    /// transaction, so the caller may be told to retry.
    #[tokio::test]
    async fn a_send_the_node_refused_keeps_its_old_answer() {
        let (provider, sent) = provider_against(SendRaw::NodeError(
            "insufficient funds for gas * price + value",
        ))
        .await;
        let result = provider.send_transaction(meta_transaction()).await;
        assert_eq!(sent.lock().unwrap().len(), 1);
        match result {
            Err(FacilitatorLocalError::ContractCall(message)) => {
                assert!(message.contains("insufficient funds"), "{message}");
            }
            other => panic!("expected the node's refusal as ContractCall, got {other:?}"),
        }
    }
}

/// Arc testnet (Circle), the parts that are decided WITHOUT an RPC.
///
/// Every constant here was read off the chain on 2026-09-16 at block
/// 62,335,077 through `https://rpc.testnet.arc.io`, and re-read from the
/// 2026-09-15 snapshot in the research evidence. A test that only compares our
/// table to itself proves nothing -- the domain separator below is the one
/// value that ties the address, the name, the version and the chain id to what
/// the contract actually answers.
#[cfg(test)]
mod arc_testnet_tests {
    use super::*;
    use alloy::dyn_abi::DynSolValue;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;

    /// `DOMAIN_SEPARATOR()` as the Arc USDC proxy returned it.
    const ARC_USDC_DOMAIN_SEPARATOR: [u8; 32] =
        hex!("361191522483d32a83e70ae7183b4b9629442c13a78bc9921d6f707911c8c6b0");
    /// Arc's documented minimum `maxFeePerGas`, and the base fee the chain has
    /// held at every reading.
    const ARC_MIN_MAX_FEE: u128 = 20 * GWEI;
    /// geth includes no tip below this, and it is what the generic arm gives
    /// every chain that has not measured its own.
    const ONE_MWEI: u128 = 1_000_000;

    fn arc_usdc() -> Address {
        USDCDeployment::by_network(Network::ArcTestnet)
            .expect("Arc testnet has a USDC deployment")
            .address()
            .try_into()
            .expect("Arc USDC is an EVM address")
    }

    fn arc_chain_id() -> u64 {
        EvmChain::try_from(Network::ArcTestnet)
            .expect("Arc is an EVM chain")
            .chain_id
    }

    /// The four fields a payer's signature commits to, checked against the one
    /// number the contract publishes. Change the address, the name, the
    /// version or the chain id and this stops matching -- which is the same
    /// moment every Arc signature would stop recovering its signer.
    #[test]
    fn arc_usdc_domain_separator_is_the_one_the_contract_publishes() {
        let (name, version) = find_known_eip712_metadata(Network::ArcTestnet, &arc_usdc())
            .expect("Arc USDC is in the static EIP-712 table");
        let domain = eip712_domain! {
            name: name,
            version: version,
            chain_id: arc_chain_id(),
            verifying_contract: arc_usdc(),
        };
        assert_eq!(
            domain.separator().0,
            ARC_USDC_DOMAIN_SEPARATOR,
            "the domain we build no longer matches DOMAIN_SEPARATOR() on Arc"
        );

        // Control: the same token under another chain's id is a different
        // domain. Without this the assertion above could be satisfied by a
        // chain id that is ignored.
        let wrong_chain = eip712_domain! {
            name: "USDC",
            version: "2",
            chain_id: 8453_u64,
            verifying_contract: arc_usdc(),
        };
        assert_ne!(wrong_chain.separator().0, ARC_USDC_DOMAIN_SEPARATOR);
    }

    /// The whole reason Arc has a fee floor of its own.
    ///
    /// Delete [`eip1559_fee_floor`]'s Arc arm and this test fails twice over:
    /// the generic arm carries `min_max_fee = 0`, so there is no floor at all,
    /// and its `fallback_base_fee` of 2 gwei prices a transaction at 4.001
    /// gwei -- a fifth of the minimum Arc accepts.
    #[test]
    fn a_settle_on_arc_is_priced_above_the_chains_documented_minimum() {
        let floor = eip1559_fee_floor(Network::ArcTestnet);
        assert!(
            floor.min_max_fee >= ARC_MIN_MAX_FEE,
            "Arc refuses a maxFeePerGas below 20 gwei; this floor offers {}",
            floor.min_max_fee
        );
        assert!(
            floor.fallback_base_fee >= ARC_MIN_MAX_FEE,
            "the base fee assumed when eth_feeHistory fails is {}, below the \
             minimum the chain accepts",
            floor.fallback_base_fee
        );

        // The node quotes a zero tip on Arc (`eth_maxPriorityFeePerGas` = 0x0),
        // so the tip contributes nothing and the cap has to come from the floor.
        let (priority, max_fee) = compute_eip1559_fees(20 * GWEI, 0, floor);
        assert!(max_fee >= ARC_MIN_MAX_FEE, "priced at {max_fee} wei");
        assert_eq!(priority, ONE_MWEI);

        // And the path a failed fee read takes: `quote_eip1559_fees` falls back
        // to `fallback_base_fee`, and the send path's error branch sets
        // `min_max_fee` directly. Both have to clear the minimum.
        let (_, from_fallback) = compute_eip1559_fees(floor.fallback_base_fee, 0, floor);
        assert!(
            from_fallback >= ARC_MIN_MAX_FEE,
            "a failed fee read prices Arc at {from_fallback} wei"
        );
        assert!(
            floor.min_max_fee > 0,
            "the send path only applies a floor on a failed read when \
             min_max_fee > 0; at zero it hands the pricing back to the default \
             estimator, which is what leaves Arc underpriced"
        );
    }

    /// Arc ships OFF, and this is the whole mechanism.
    ///
    /// `ProviderCache::from_env` walks `Network::variants()` and keeps only the
    /// networks that answered with a provider; `/supported` then iterates that
    /// map. With `RPC_URL_ARC_TESTNET` unset there is no provider, so Arc
    /// appears in the enum, in `variants()`, in the token tables and in the
    /// asset allow-list while being served by nothing -- and `/supported`
    /// never names it. Turning Arc on is a separate change, in the deployment's
    /// configuration, not in this code.
    #[tokio::test]
    async fn arc_is_served_by_nothing_until_its_rpc_url_is_configured() {
        assert_eq!(
            from_env::rpc_env_name_from_network(Network::ArcTestnet),
            "RPC_URL_ARC_TESTNET"
        );
        std::env::remove_var("RPC_URL_ARC_TESTNET");
        let provider = EvmProvider::from_env(Network::ArcTestnet)
            .await
            .expect("an unconfigured network is not an error");
        assert!(
            provider.is_none(),
            "Arc built a provider with no RPC URL configured; it would then be \
             advertised by /supported the moment this ships"
        );
    }

    /// Arc raises the CAP, not the tip. Circle permits a zero tip and the node
    /// quotes zero; there is nothing measured here to justify more, and a tip
    /// floor copied from a chain that earned it is what drained the mainnet
    /// signer in four days.
    #[test]
    fn arc_keeps_the_generic_one_mwei_tip() {
        assert_eq!(
            eip1559_fee_floor(Network::ArcTestnet).min_priority,
            ONE_MWEI,
            "a tip floor above 1 mwei has to be measured for the chain it is on"
        );
    }

    /// A 6492-wrapped signature as it arrives on the wire: the ABI-encoded
    /// `(factory, factoryCalldata, innerSig)` tuple followed by the magic
    /// suffix.
    pub(super) fn wire_eip6492_signature() -> Vec<u8> {
        let mut bytes = DynSolValue::Tuple(vec![
            DynSolValue::Address(address!("0x00000000000000000000000000000000000f4c70")),
            DynSolValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
            DynSolValue::Bytes(vec![0x11; 65]),
        ])
        .abi_encode_params();
        bytes.extend_from_slice(&EIP6492_MAGIC_SUFFIX);
        bytes
    }

    /// Any address; the gate does not look at it, it only reports it.
    const SOME_PAYER: Address = address!("0x1111111111111111111111111111111111111111");

    fn gate(network: Network, signature: Vec<u8>) -> Result<(), FacilitatorLocalError> {
        assert_signature_scheme_supported(network, EvmAddress(SOME_PAYER), &EvmSignature(signature))
    }

    /// The universal signature validator has no code on Arc (measured: 0
    /// bytes). Calling it anyway does not refuse the signature -- an
    /// `eth_call` to an address with no code returns empty data, and the
    /// caller gets a decode failure that reads like a broken token. The
    /// refusal has to happen before the VALIDATOR call, and it has to be the
    /// same refusal every time.
    ///
    /// And on Arc it IS before every `eth_call` the payment makes, which the
    /// name understates. Measured on this tree: the gate sits at the top of
    /// `assert_valid_payment`'s signature checks and `assert_enough_balance`
    /// comes after it, while `assert_domain` resolves Arc USDC out of the
    /// static table and never reaches the node. Point the fixture's node at a
    /// mock that errors on EVERY `eth_call` and the four payment tests fail
    /// while both 6492 tests stay green -- the refusal needs nothing from the
    /// chain.
    ///
    /// The name still says "validator" because that is the guarantee the gate
    /// owes on every chain: on one whose token is NOT in the static table,
    /// `assert_domain` would fall back to an on-chain `name()`/`version()`
    /// read before this runs.
    #[test]
    fn eip6492_is_refused_on_arc_before_the_validator_call() {
        assert!(!has_eip6492_validator(Network::ArcTestnet));

        let wire = wire_eip6492_signature();
        assert!(
            matches!(
                StructuredSignature::try_from(wire.clone()),
                Ok(StructuredSignature::EIP6492 { .. })
            ),
            "premise: the wire bytes really are read as 6492"
        );

        let error = gate(Network::ArcTestnet, wire)
            .expect_err("Arc must refuse a counterfactual signature");
        match error {
            FacilitatorLocalError::InvalidSignature(_, message) => {
                assert!(
                    message.contains("EIP-6492") && message.contains("arc-testnet"),
                    "the refusal must name the scheme and the chain: {message}"
                );
            }
            other => panic!("expected an invalid-signature verdict, got {other:?}"),
        }
    }

    /// The premise behind the ordering, not the ordering itself.
    ///
    /// An EIP-6492 envelope is never 65 bytes, so whichever of the two checks
    /// runs first is the one that answers. While the gate sat after
    /// `SignedMessage::extract`, the length rule had already refused the
    /// envelope as `invalid_signature_length` and the gate never ran -- which
    /// is why deleting it from both endpoints changed no test.
    ///
    /// **This test cannot catch a reorder**, and saying otherwise would be
    /// worse than not testing it: it calls the gate as a free function, so the
    /// order of the two checks inside `assert_valid_payment` is invisible from
    /// here. The reorder is caught where it is observable, by the
    /// `invalid_signature_length` assertion inside
    /// `settle_refuses_a_counterfactual_signature_and_sends_nothing` and its
    /// `verify` twin, which go through the endpoints. What is pinned here is
    /// the premise those two rest on: that the envelope is not 65 bytes, and
    /// that the gate alone refuses it without mentioning length.
    #[test]
    fn the_gate_speaks_before_the_sixty_five_byte_rule() {
        let wire = wire_eip6492_signature();
        assert_ne!(wire.len(), 65, "premise: a 6492 envelope is not 65 bytes");
        let message = gate(Network::ArcTestnet, wire)
            .expect_err("Arc refuses it")
            .to_string();
        assert!(
            !message.contains("invalid_signature_length"),
            "the length rule answered first, so the gate is unreachable again: {message}"
        );
    }

    /// The refusal is read by a person holding a 400, so it has to read like a
    /// sentence.
    ///
    /// It shipped with two runs of EIGHTEEN spaces in the middle of it: the
    /// indentation of a `\`-continued literal whose backslash was lost on the
    /// way into the file. The compiler is happy either way and every assertion
    /// about the message used `contains`, so nothing noticed. This is the
    /// cheapest check that would have.
    #[test]
    fn no_double_spaces_in_the_eip6492_refusal() {
        let error = gate(Network::ArcTestnet, wire_eip6492_signature())
            .expect_err("Arc refuses a counterfactual signature");
        let message = error.to_string();
        assert!(
            !message.contains("  "),
            "the refusal carries a run of spaces, so a continuation was eaten: {message:?}"
        );
        // And it is still the whole sentence, not a fragment that happens to
        // have no double space in it.
        assert!(message.ends_with("on this network."), "{message:?}");
        // Generic: the gate serves any chain without a validator, not only
        // Arc, so the refusal names no token and no chain but the one asked.
        assert!(!message.contains("USDC"), "{message:?}");
        assert!(
            message.contains("the universal signature validator"),
            "{message:?}"
        );
    }

    /// Only 6492 is gated, and only on Arc. An ordinary EOA payment -- which
    /// is the whole of the first launch here -- travels the EIP-1271 branch
    /// and must be untouched; so must every other chain's 6492 support.
    #[test]
    fn the_gate_closes_on_nothing_else() {
        // A plain 65-byte EOA signature, on the chain that has no validator.
        assert!(gate(Network::ArcTestnet, vec![0x33; 65]).is_ok());

        for network in Network::variants() {
            if matches!(network, Network::Arc | Network::ArcTestnet) {
                continue;
            }
            assert!(
                has_eip6492_validator(*network),
                "{network} lost its 6492 support without a measurement saying so"
            );
        }
        // And the same counterfactual envelope passes the gate everywhere else.
        assert!(gate(Network::Base, wire_eip6492_signature()).is_ok());
    }

    /// Circle Gateway announces the SAME `scheme` and the SAME network
    /// (`exact`, `eip155:5042002`) while signing against a different domain:
    /// `GatewayWalletBatched` version 1, verified by the Gateway Wallet
    /// contract rather than by USDC. `scheme + network` is therefore not
    /// enough to decide two authorizations are interchangeable.
    ///
    /// Three independent things refuse it, and the test pins all three,
    /// because any one of them alone is a single point of failure.
    #[test]
    fn a_gateway_authorization_cannot_pass_as_a_direct_arc_payment() {
        let payer = PrivateKeySigner::random();
        let authorization = TransferWithAuthorization {
            from: payer.address(),
            to: address!("0x2222222222222222222222222222222222222222"),
            value: U256::from(10_000u64),
            validAfter: U256::ZERO,
            validBefore: U256::from(u64::MAX),
            nonce: FixedBytes([0x42; 32]),
        };

        let gateway_wallet = address!("0x0077777d7eba4688bdef3e311b846f25870a19b9");
        let gateway_domain = eip712_domain! {
            name: "GatewayWalletBatched",
            version: "1",
            chain_id: arc_chain_id(),
            verifying_contract: gateway_wallet,
        };
        let (name, version) = find_known_eip712_metadata(Network::ArcTestnet, &arc_usdc())
            .expect("Arc USDC is in the static table");
        let direct_domain = eip712_domain! {
            name: name,
            version: version,
            chain_id: arc_chain_id(),
            verifying_contract: arc_usdc(),
        };

        // 1. The digests are different, so the signature the payer produced for
        //    Gateway recovers somebody else against our domain. The signature
        //    stays the authority; nothing in `extra` can move it.
        let gateway_digest = authorization.eip712_signing_hash(&gateway_domain);
        let direct_digest = authorization.eip712_signing_hash(&direct_domain);
        assert_ne!(gateway_digest, direct_digest);

        let signature = payer.sign_hash_sync(&gateway_digest).expect("signs");
        assert_eq!(
            signature.recover_address_from_prehash(&gateway_digest).ok(),
            Some(payer.address()),
            "premise: the signature is valid for the domain it was made for"
        );
        assert_ne!(
            signature.recover_address_from_prehash(&direct_digest).ok(),
            Some(payer.address()),
            "a Gateway authorization must not recover its payer under the USDC \
             domain -- if it did, a batched authorization would settle here as \
             a direct transfer"
        );

        // 2. The static table wins over anything the client sends, so naming
        //    Gateway's domain in `extra` does not make the digest move.
        assert_eq!(
            find_known_eip712_metadata(Network::ArcTestnet, &arc_usdc()),
            Some(("USDC".to_string(), "2".to_string()))
        );

        // 3. And the Gateway Wallet is not an asset this network accepts, which
        //    is checked before the RPC layer is touched at all.
        assert!(!crate::network::is_supported_asset(
            Network::ArcTestnet,
            &gateway_wallet.into()
        ));
    }
}

/// Arc testnet against a node, with the node's answers pinned to what the real
/// one gives.
///
/// The four shapes a settle can end in -- a confirmed receipt, a receipt that
/// reverted, a receipt that never arrives, and a revert caught in estimation --
/// plus Arc's own wrinkle: a USDC movement emits TWO `Transfer` logs, the
/// ERC-20 one from the token and a native one, in 18 decimals, from the system
/// emitter.
///
/// `eth_chainId`, `baseFeePerGas`, `eth_maxPriorityFeePerGas` and the
/// `Blocked address` revert string are the values Arc returned on 2026-09-16 at
/// block 62,335,077, not invented ones.
#[cfg(test)]
mod arc_node_fixtures {
    use super::*;
    use crate::erc8004::proof::{unix_now_secs, verify_payment_facts, ProofRejection};
    use alloy::network::EthereumWallet;
    use alloy::primitives::keccak256;
    use alloy::providers::ProviderBuilder;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;
    use axum::{extract::State, routing::post, Json as AxumJson, Router};
    use serde_json::{json, Value};

    /// `eth_chainId` on Arc testnet.
    /// The base fee Arc has held at every reading: 20 gwei.
    const ARC_BASE_FEE_HEX: &str = "0x4a817c800";
    /// Circle seeds a BLOCKED address at genesis -- index 1 of Foundry's public
    /// test mnemonic -- and every value transfer to or from it reverts. An
    /// end-to-end test that reaches for the usual Anvil accounts out of habit
    /// lands on it and reads like a bug of ours.
    const BLOCKED_ADDRESS: Address = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
    /// The revert the chain answers for that address, verbatim.
    const BLOCKED_REVERT: &str = "execution reverted: Blocked address";
    /// Arc's system emitter for native USDC movements. Its `Transfer` carries
    /// 18 decimals for the same payment the token reports in 6.
    const SYSTEM_EMITTER: Address = address!("0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE");

    const BLOCK: u64 = 0x3b7_2865;
    const TIP: u64 = BLOCK - 10;
    const TX: [u8; 32] = [0xa7; 32];
    const BLOCK_HASH: [u8; 32] = [0xbc; 32];
    const PAYEE: Address = address!("0x2222222222222222222222222222222222222222");
    /// 0.01 USDC in the 6-decimal ERC-20 view.
    const AMOUNT: u64 = 10_000;
    /// The same payment as the native balance sees it: 18 decimals.
    const NATIVE_AMOUNT: u128 = AMOUNT as u128 * 1_000_000_000_000;

    #[derive(Clone, Copy, PartialEq)]
    enum Receipt {
        Confirmed,
        Reverted,
        /// The node never has it, as after a broadcast we lose track of.
        Never,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Estimate {
        Ok,
        /// `eth_estimateGas` reverts, which is what a payment to or from the
        /// blocked address does.
        BlockedAddress,
    }

    #[derive(Clone)]
    struct ArcNode {
        chain_id: u64,
        payer: Address,
        receipt: Receipt,
        estimate: Estimate,
        /// Frozen at fixture time. A proof's timestamp has to equal the
        /// block's to the second, so the node and the test have to read the
        /// same number -- not two calls to the clock a second apart.
        block_timestamp: u64,
        /// Broadcasts the node was asked to make.
        broadcasts: Arc<AtomicUsize>,
    }

    fn arc_usdc() -> Address {
        USDCDeployment::by_network(Network::ArcTestnet)
            .expect("Arc has a USDC deployment")
            .address()
            .try_into()
            .expect("an EVM address")
    }

    /// One `Transfer` log. `emitter` is what tells the token's event from the
    /// chain's own: the topic is identical in both.
    fn transfer_log(emitter: Address, from: Address, to: Address, value: U256) -> Value {
        json!({
            "address": emitter,
            "topics": [
                keccak256("Transfer(address,address,uint256)"),
                from.into_word(),
                to.into_word(),
            ],
            "data": format!("0x{}", hex::encode(value.to_be_bytes::<32>())),
            "blockNumber": format!("{BLOCK:#x}"),
            "blockHash": format!("0x{}", hex::encode(BLOCK_HASH)),
            "transactionHash": format!("0x{}", hex::encode(TX)),
            "transactionIndex": "0x0",
            "logIndex": "0x0",
            "removed": false
        })
    }

    fn receipt_json(payer: Address, status: &str) -> Value {
        json!({
            "transactionHash": format!("0x{}", hex::encode(TX)),
            "transactionIndex": "0x0",
            "blockHash": format!("0x{}", hex::encode(BLOCK_HASH)),
            "blockNumber": format!("{BLOCK:#x}"),
            "from": address!("0x0000000000000000000000000000000000000001"),
            "to": arc_usdc(),
            "cumulativeGasUsed": "0xfde8",
            "gasUsed": "0xfde8",
            "contractAddress": null,
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "status": status,
            "type": "0x2",
            "effectiveGasPrice": ARC_BASE_FEE_HEX,
            // BOTH logs, the way Arc emits them.
            "logs": [
                transfer_log(arc_usdc(), payer, PAYEE, U256::from(AMOUNT)),
                transfer_log(SYSTEM_EMITTER, payer, PAYEE, U256::from(NATIVE_AMOUNT)),
            ]
        })
    }

    fn block_json(number: u64, timestamp: u64) -> Value {
        let zero32 = format!("0x{}", hex::encode([0u8; 32]));
        json!({
            "hash": format!("0x{}", hex::encode(BLOCK_HASH)),
            "parentHash": zero32,
            "sha3Uncles": zero32,
            "miner": format!("0x{}", hex::encode([0u8; 20])),
            "stateRoot": zero32,
            "transactionsRoot": zero32,
            "receiptsRoot": zero32,
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "difficulty": "0x0",
            "number": format!("{number:#x}"),
            "gasLimit": "0x1c9c380",
            "gasUsed": "0x16de71",
            "timestamp": format!("{timestamp:#x}"),
            "extraData": "0x",
            "mixHash": zero32,
            "nonce": "0x0000000000000000",
            "baseFeePerGas": ARC_BASE_FEE_HEX,
            "totalDifficulty": "0x0",
            "size": "0x220",
            "transactions": [],
            "uncles": []
        })
    }

    fn answer(node: &ArcNode, req: &Value) -> Value {
        let id = req.get("id").cloned().unwrap_or(json!(1));
        let error = |message: &str| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": 3, "message": message},
            })
        };
        let result = match req["method"].as_str().unwrap_or_default() {
            "eth_chainId" => json!(format!("{:#x}", node.chain_id)),
            "eth_getCode" => json!("0x"),
            "eth_getTransactionCount" => json!("0x0"),
            // Arc quotes a zero tip; the floor is what has to carry the cap.
            "eth_maxPriorityFeePerGas" => json!("0x0"),
            "eth_feeHistory" => json!({
                "oldestBlock": format!("{TIP:#x}"),
                "baseFeePerGas": [ARC_BASE_FEE_HEX, ARC_BASE_FEE_HEX],
                "gasUsedRatio": [0.5],
                "reward": [["0x0"]],
            }),
            "eth_estimateGas" => match node.estimate {
                Estimate::Ok => json!("0xfde8"),
                Estimate::BlockedAddress => return error(BLOCKED_REVERT),
            },
            // `balanceOf`, the only contract read a settle makes.
            "eth_call" => json!(format!(
                "0x{}",
                hex::encode(U256::from(AMOUNT * 100).to_be_bytes::<32>())
            )),
            "eth_sendRawTransaction" => {
                node.broadcasts.fetch_add(1, Ordering::SeqCst);
                json!(format!("0x{}", hex::encode(TX)))
            }
            "eth_blockNumber" => json!(format!("{TIP:#x}")),
            "eth_getTransactionReceipt" => match node.receipt {
                Receipt::Confirmed => receipt_json(node.payer, "0x1"),
                Receipt::Reverted => receipt_json(node.payer, "0x0"),
                Receipt::Never => Value::Null,
            },
            "eth_getBlockByNumber" => {
                let number = req["params"][0]
                    .as_str()
                    .and_then(|n| u64::from_str_radix(n.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(TIP);
                block_json(number, node.block_timestamp)
            }
            _ => Value::Null,
        };
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    async fn rpc(State(node): State<ArcNode>, AxumJson(body): AxumJson<Value>) -> AxumJson<Value> {
        AxumJson(match &body {
            Value::Array(reqs) => Value::Array(reqs.iter().map(|r| answer(&node, r)).collect()),
            req => answer(&node, req),
        })
    }

    async fn spawn(node: ArcNode) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/", post(rpc)).with_state(node);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/")
    }

    /// The wire body of a 0.01 USDC payment on Arc, built the way a client must
    /// build it: explicit contract, explicit amount, explicit domain.
    ///
    /// `signature` is a parameter rather than something this builds, so the
    /// same body can carry the payer's real EIP-712 signature or a
    /// counterfactual EIP-6492 envelope. `/verify` and `/settle` take the same
    /// shape, so both requests come from here and cannot drift apart.
    fn request_json(payer: &PrivateKeySigner, to: Address, signature: Vec<u8>) -> Value {
        let valid_before = unix_now_secs() + 300;
        json!({
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": "arc-testnet",
                "payload": {
                    "signature": format!("0x{}", hex::encode(&signature)),
                    "authorization": {
                        "from": payer.address(),
                        "to": to,
                        "value": AMOUNT.to_string(),
                        "validAfter": "0",
                        "validBefore": valid_before.to_string(),
                        "nonce": format!("0x{}", hex::encode([0x42u8; 32])),
                    }
                }
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "arc-testnet",
                "maxAmountRequired": AMOUNT.to_string(),
                "resource": "https://example.com/paid",
                "description": "",
                "mimeType": "application/json",
                "payTo": to,
                "maxTimeoutSeconds": 300,
                "asset": arc_usdc(),
                "extra": {"name": "USDC", "version": "2"},
            }
        })
    }

    /// The payer's real EIP-712 signature over that body, under Arc's USDC
    /// domain.
    fn eip712_signature(payer: &PrivateKeySigner, body: &Value) -> Vec<u8> {
        let network: Network = body["paymentRequirements"]["network"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let (name, version) = find_known_eip712_metadata(network, &arc_usdc())
            .expect("Arc USDC is in the static table");
        let domain = eip712_domain! {
            name: name,
            version: version,
            chain_id: EvmChain::try_from(network).unwrap().chain_id,
            verifying_contract: arc_usdc(),
        };
        let authorization = &body["paymentPayload"]["payload"]["authorization"];
        let valid_before: u64 = authorization["validBefore"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .expect("validBefore is a decimal string");
        let to: Address = authorization["to"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .expect("to is an address");
        let transfer = TransferWithAuthorization {
            from: payer.address(),
            to,
            value: U256::from(AMOUNT),
            validAfter: U256::ZERO,
            validBefore: U256::from(valid_before),
            nonce: FixedBytes([0x42; 32]),
        };
        payer
            .sign_hash_sync(&transfer.eip712_signing_hash(&domain))
            .expect("signs")
            .as_bytes()
            .to_vec()
    }

    /// A correctly signed payment, as `/settle` receives it.
    fn settle_request(payer: &PrivateKeySigner, to: Address) -> SettleRequest {
        let mut body = request_json(payer, to, Vec::new());
        let signature = eip712_signature(payer, &body);
        body["paymentPayload"]["payload"]["signature"] =
            json!(format!("0x{}", hex::encode(signature)));
        serde_json::from_value(body).expect("the settle request parses")
    }

    /// The same payment carrying a COUNTERFACTUAL EIP-6492 envelope instead of
    /// an EOA signature, as `/settle` receives it.
    fn settle_request_6492(payer: &PrivateKeySigner, to: Address) -> SettleRequest {
        serde_json::from_value(request_json(
            payer,
            to,
            super::arc_testnet_tests::wire_eip6492_signature(),
        ))
        .expect("the settle request parses")
    }

    /// Ditto, as `/verify` receives it. Same body: the two endpoints take the
    /// same shape, and the gate has to be on both.
    fn verify_request_6492(payer: &PrivateKeySigner, to: Address) -> VerifyRequest {
        serde_json::from_value(request_json(
            payer,
            to,
            super::arc_testnet_tests::wire_eip6492_signature(),
        ))
        .expect("the verify request parses")
    }

    struct Fixture {
        provider: EvmProvider,
        broadcasts: Arc<AtomicUsize>,
        url: String,
        block_timestamp: u64,
    }

    async fn fixture(receipt: Receipt, estimate: Estimate, payer: Address) -> Fixture {
        fixture_for(Network::ArcTestnet, receipt, estimate, payer).await
    }

    async fn fixture_for(
        network: Network,
        receipt: Receipt,
        estimate: Estimate,
        payer: Address,
    ) -> Fixture {
        let broadcasts = Arc::new(AtomicUsize::new(0));
        let block_timestamp = unix_now_secs() - 5;
        let url = spawn(ArcNode {
            chain_id: EvmChain::try_from(network).unwrap().chain_id,
            payer,
            receipt,
            estimate,
            block_timestamp,
            broadcasts: broadcasts.clone(),
        })
        .await;
        // `eip1559 = true`, as Arc is configured: the pricing under test is the
        // fee-history negotiation, not a legacy `eth_gasPrice`.
        let provider = EvmProvider::try_new(
            EthereumWallet::from(PrivateKeySigner::random()),
            &url,
            true,
            network,
        )
        .await
        .expect("provider");
        Fixture {
            provider,
            broadcasts,
            url,
            block_timestamp,
        }
    }

    /// The node quotes 20 gwei base and a zero tip. The cap the facilitator
    /// sets is what the node reserves against the signer's balance, and it has
    /// to clear Arc's minimum.
    #[tokio::test]
    async fn arc_prices_a_transaction_off_the_nodes_own_numbers() {
        let f = fixture(Receipt::Confirmed, Estimate::Ok, Address::ZERO).await;
        let quote = f.provider.quote_eip1559_fees().await.expect("quote");
        assert_eq!(quote.base_fee, 20 * GWEI);
        assert_eq!(
            quote.priority, 1_000_000,
            "the node quoted zero; 1 mwei is the floor"
        );
        assert_eq!(quote.max_fee, 2 * 20 * GWEI + 1_000_000);
        assert!(
            quote.max_fee >= 20 * GWEI,
            "below Arc's minimum maxFeePerGas"
        );
        assert_eq!(
            f.provider.quote_fee_cap().await.expect("cap"),
            quote.max_fee
        );
    }

    /// The ordinary path: a confirmed receipt is a successful settle, and the
    /// hash it reports is the one the node handed back.
    #[tokio::test]
    async fn a_confirmed_receipt_settles() {
        let payer = PrivateKeySigner::random();
        let f = fixture(Receipt::Confirmed, Estimate::Ok, payer.address()).await;
        let response = f
            .provider
            .settle(&settle_request(&payer, PAYEE))
            .await
            .expect("the settle reaches a verdict");
        assert!(response.success);
        assert_eq!(response.network, Network::ArcTestnet);
        assert_eq!(
            response.transaction,
            Some(TransactionHash::Evm(TX)),
            "the response must carry the hash the node returned"
        );
        assert_eq!(f.broadcasts.load(Ordering::SeqCst), 1);
    }

    /// A mined transaction is not a successful one.
    ///
    /// Arc finalises on inclusion, so a transfer the token reverted still comes
    /// back with a real receipt and a real hash. The send path refuses it
    /// before `settle` reaches `receipt.status()`, and the refusal carries the
    /// hash and the chain -- the caller has to be able to look up what actually
    /// happened, and must never be told a reverted transfer settled.
    #[tokio::test]
    async fn a_reverted_receipt_is_refused_and_names_its_transaction() {
        let payer = PrivateKeySigner::random();
        let f = fixture(Receipt::Reverted, Estimate::Ok, payer.address()).await;
        let error = f
            .provider
            .settle(&settle_request(&payer, PAYEE))
            .await
            .expect_err("a reverted transaction is not a settlement");
        let message = format!("{error}");
        assert!(
            matches!(error, FacilitatorLocalError::ContractCall(_)),
            "expected a contract-call refusal, got {error:?}"
        );
        assert!(
            message.contains(&hex::encode(TX)) && message.contains("arc-testnet"),
            "the refusal must name the transaction and the chain: {message}"
        );
        assert_eq!(f.broadcasts.load(Ordering::SeqCst), 1);
    }

    /// Arc confirms on inclusion, but the reply to a broadcast is not a receipt.
    /// When the receipt never arrives the settle must report the hash rather
    /// than either success or a plain failure: a caller told "failed" retries,
    /// and a retry of a payment that did land is a second debit.
    #[tokio::test]
    async fn a_settle_whose_receipt_never_arrives_reports_its_hash() {
        let payer = PrivateKeySigner::random();
        let f = fixture(Receipt::Never, Estimate::Ok, payer.address()).await;
        std::env::set_var("TX_RECEIPT_TIMEOUT_SECS", "1");
        let result = f.provider.settle(&settle_request(&payer, PAYEE)).await;
        std::env::remove_var("TX_RECEIPT_TIMEOUT_SECS");

        match result {
            Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                assert_eq!(tx, TransactionHash::Evm(TX));
                assert_eq!(network, Network::ArcTestnet);
            }
            other => panic!(
                "a broadcast with no receipt must report SettlementUnconfirmed with its \
                 hash; got {other:?}"
            ),
        }
        assert_eq!(f.broadcasts.load(Ordering::SeqCst), 1);
    }

    /// The EIP-6492 gate, through `/settle` rather than through the helper.
    ///
    /// The unit test one module up calls `assert_signature_scheme_supported`
    /// directly, so it stays green with the gate DELETED from the endpoint --
    /// which is exactly the mutant that survived the whole suite. This one goes
    /// through the real `Facilitator::settle`, so removing the call there turns
    /// it red.
    ///
    /// Asserting the VARIANT is the load-bearing part. Without the gate the
    /// settle still fails -- it walks into the counterfactual path and the node
    /// gives it nothing to decode -- but it fails as `ContractCall`, blaming
    /// the chain for a facilitator-side gap. `is_err()` would not tell the two
    /// apart.
    #[tokio::test]
    async fn settle_refuses_a_counterfactual_signature_and_sends_nothing() {
        let payer = PrivateKeySigner::random();
        let f = fixture(Receipt::Confirmed, Estimate::Ok, payer.address()).await;
        let error = f
            .provider
            .settle(&settle_request_6492(&payer, PAYEE))
            .await
            .expect_err("Arc must refuse a counterfactual signature on settle");
        match error {
            FacilitatorLocalError::InvalidSignature(_, ref message) => {
                assert!(
                    message.contains("EIP-6492") && message.contains("arc-testnet"),
                    "the refusal must name the scheme and the chain: {message}"
                );
                // THE ORDERING, pinned where it is observable. The gate has to
                // run ahead of `assert_valid_payment`'s 65-byte rule; a 6492
                // envelope is never 65 bytes, so whichever check runs first
                // answers. Put the gate back after it -- where it sat, and
                // where it was unreachable -- and this is the assertion that
                // goes red, naming the reason.
                assert!(
                    !message.contains("invalid_signature_length"),
                    "the 65-byte rule answered first, so the gate is unreachable \
                     again and only its position changed: {message}"
                );
            }
            other => panic!(
                "settle must refuse this as an invalid signature, not as a \
                 contract failure; got {other:?}"
            ),
        }
        assert_eq!(
            f.broadcasts.load(Ordering::SeqCst),
            0,
            "a signature we cannot validate must never reach the wire"
        );
    }

    /// The same gate on `/verify`, which is its own mutant: the two call sites
    /// are independent and deleting either one alone left the suite green.
    #[tokio::test]
    async fn verify_refuses_a_counterfactual_signature_and_sends_nothing() {
        let payer = PrivateKeySigner::random();
        let f = fixture(Receipt::Confirmed, Estimate::Ok, payer.address()).await;
        let error = f
            .provider
            .verify(&verify_request_6492(&payer, PAYEE))
            .await
            .expect_err("Arc must refuse a counterfactual signature on verify");
        match error {
            FacilitatorLocalError::InvalidSignature(_, ref message) => {
                assert!(
                    message.contains("EIP-6492") && message.contains("arc-testnet"),
                    "the refusal must name the scheme and the chain: {message}"
                );
                // THE ORDERING, pinned where it is observable. The gate has to
                // run ahead of `assert_valid_payment`'s 65-byte rule; a 6492
                // envelope is never 65 bytes, so whichever check runs first
                // answers. Put the gate back after it -- where it sat, and
                // where it was unreachable -- and this is the assertion that
                // goes red, naming the reason.
                assert!(
                    !message.contains("invalid_signature_length"),
                    "the 65-byte rule answered first, so the gate is unreachable \
                     again and only its position changed: {message}"
                );
            }
            other => panic!(
                "verify must refuse this as an invalid signature, not as a \
                 contract failure; got {other:?}"
            ),
        }
        assert_eq!(
            f.broadcasts.load(Ordering::SeqCst),
            0,
            "verify never broadcasts, and must not start here"
        );
    }

    /// Circle's genesis blocked address, reached in estimation. Nothing may go
    /// on the wire: a broadcast here would burn a nonce on a transaction that
    /// cannot be mined, and every settle behind it waits.
    #[tokio::test]
    async fn a_transfer_the_chain_refuses_never_reaches_the_wire() {
        let payer = PrivateKeySigner::random();
        let f = fixture(
            Receipt::Confirmed,
            Estimate::BlockedAddress,
            payer.address(),
        )
        .await;
        let error = f
            .provider
            .settle(&settle_request(&payer, BLOCKED_ADDRESS))
            .await
            .expect_err("a transfer to a blocked address cannot settle");
        assert!(
            format!("{error}").contains("Blocked address"),
            "the chain's own reason must survive to the caller: {error}"
        );
        assert_eq!(
            f.broadcasts.load(Ordering::SeqCst),
            0,
            "gas estimation reverted, so no nonce was reserved and nothing was sent"
        );
    }

    /// Arc's double event, and the reason the receipt reader keys on the log's
    /// EMITTER rather than on the `Transfer` topic.
    ///
    /// The same payment appears twice in one receipt: 10,000 units from the
    /// token, and 10,000,000,000,000,000 from the chain's system emitter. Both
    /// carry the identical topic and the identical `from`/`to`. A reader that
    /// matched on the topic would see the 18-decimal figure as a transfer of
    /// ten billion USDC.
    ///
    /// Both Arc networks: mainnet and testnet share the token address and the
    /// system emitter, so the rule has to hold on each of them.
    #[tokio::test]
    async fn the_native_system_event_is_not_read_as_the_payment() {
        for network in [Network::Arc, Network::ArcTestnet] {
            let payer = PrivateKeySigner::random();
            let f = fixture_for(network, Receipt::Confirmed, Estimate::Ok, payer.address()).await;
            let rpc = ProviderBuilder::new().connect(&f.url).await.expect("rpc");

            let proof = |amount: u128| {
                ProofOfPayment::new(
                    TransactionHash::Evm(TX),
                    BLOCK,
                    network,
                    MixedAddress::from(payer.address()),
                    MixedAddress::from(PAYEE),
                    TokenAmount::from(amount),
                    MixedAddress::from(arc_usdc()),
                    f.block_timestamp,
                )
            };

            // The ERC-20 amount, from the token's own log: accepted.
            let facts = verify_payment_facts(&rpc, network, &proof(AMOUNT as u128), 900)
                .await
                .unwrap_or_else(|e| panic!("{network}: the token's own Transfer proves it: {e:?}"));
            assert_eq!(facts.payer, payer.address(), "{network}");
            assert_eq!(facts.payee, PAYEE, "{network}");
            assert_eq!(facts.token, arc_usdc(), "{network}");

            // The native 18-decimal amount, which only the system emitter
            // reports: refused, because that log is not the token's.
            let rejection = verify_payment_facts(&rpc, network, &proof(NATIVE_AMOUNT), 900)
                .await
                .expect_err("the system emitter's event is not the token's");
            assert!(
                matches!(rejection, ProofRejection::TransferNotFound),
                "{network}: expected the native event to be ignored, got {rejection:?}"
            );
        }
    }
    fn mainnet_body(payer: &PrivateKeySigner, signature: Vec<u8>) -> Value {
        let mut body = request_json(payer, PAYEE, signature);
        body["paymentPayload"]["network"] = json!("arc");
        body["paymentRequirements"]["network"] = json!("arc");
        body
    }

    #[test]
    fn arc_mainnet_domain_matches_the_live_contract_and_differs_from_testnet() {
        let chain = EvmChain::try_from(Network::Arc).unwrap();
        assert_eq!(chain.chain_id, 5042);
        let (name, version) = find_known_eip712_metadata(Network::Arc, &arc_usdc()).unwrap();
        let domain = eip712_domain! {
            name: name,
            version: version,
            chain_id: chain.chain_id,
            verifying_contract: arc_usdc(),
        };
        // rpc.mainnet.arc.io, block 21183711, 2026-09-16. Public domain hash.
        assert_eq!(
            hex::encode(domain.separator()),
            "940506929bba468048a19b567f4f0d534714bc06604b5c3017e5d16785ccdf84"
        );
        let testnet = eip712_domain! {
            name: "USDC", version: "2", chain_id: 5042002_u64, verifying_contract: arc_usdc(),
        };
        assert_ne!(domain.separator(), testnet.separator());
        assert!(!has_eip6492_validator(Network::Arc));
        assert_eq!(eip1559_fee_floor(Network::Arc).min_max_fee, 20 * GWEI);
    }

    #[tokio::test]
    async fn arc_mainnet_settles_with_its_own_domain() {
        let payer = PrivateKeySigner::random();
        let f = fixture_for(
            Network::Arc,
            Receipt::Confirmed,
            Estimate::Ok,
            payer.address(),
        )
        .await;
        let mut body = mainnet_body(&payer, Vec::new());
        body["paymentPayload"]["payload"]["signature"] = json!(format!(
            "0x{}",
            hex::encode(eip712_signature(&payer, &body))
        ));
        let req: SettleRequest = serde_json::from_value(body).unwrap();
        let response = f.provider.settle(&req).await.expect("mainnet settles");
        assert!(response.success);
        assert_eq!(response.network, Network::Arc);
        assert_eq!(response.transaction, Some(TransactionHash::Evm(TX)));
        assert_eq!(f.broadcasts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn arc_mainnet_rejects_a_testnet_signature_without_broadcasting() {
        let payer = PrivateKeySigner::random();
        let f = fixture_for(
            Network::Arc,
            Receipt::Confirmed,
            Estimate::Ok,
            payer.address(),
        )
        .await;
        let wrong_signature = eip712_signature(&payer, &request_json(&payer, PAYEE, Vec::new()));
        let req: SettleRequest =
            serde_json::from_value(mainnet_body(&payer, wrong_signature)).unwrap();
        let error = f
            .provider
            .settle(&req)
            .await
            .expect_err("testnet domain is not mainnet");
        assert!(
            matches!(error, FacilitatorLocalError::InvalidSignature(_, _)),
            "{error:?}"
        );
        assert_eq!(f.broadcasts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn arc_mainnet_refuses_6492_in_verify_and_settle_without_broadcasting() {
        let payer = PrivateKeySigner::random();
        let f = fixture_for(
            Network::Arc,
            Receipt::Confirmed,
            Estimate::Ok,
            payer.address(),
        )
        .await;
        let body = mainnet_body(&payer, super::arc_testnet_tests::wire_eip6492_signature());
        let verify: VerifyRequest = serde_json::from_value(body.clone()).unwrap();
        let settle: SettleRequest = serde_json::from_value(body).unwrap();
        for error in [
            f.provider.verify(&verify).await.unwrap_err(),
            f.provider.settle(&settle).await.unwrap_err(),
        ] {
            match error {
                FacilitatorLocalError::InvalidSignature(_, message) => {
                    assert!(message.contains("EIP-6492"), "{message}");
                    assert!(!message.contains("invalid_signature_length"), "{message}");
                }
                other => panic!("wrong refusal: {other:?}"),
            }
        }
        assert_eq!(f.broadcasts.load(Ordering::SeqCst), 0);
    }

    /// An RPC answering `chain_id`, for the admission tests below.
    async fn node_answering(chain_id: u64) -> (String, Arc<AtomicUsize>) {
        let broadcasts = Arc::new(AtomicUsize::new(0));
        let url = spawn(ArcNode {
            chain_id,
            payer: Address::ZERO,
            receipt: Receipt::Confirmed,
            estimate: Estimate::Ok,
            block_timestamp: unix_now_secs(),
            broadcasts: broadcasts.clone(),
        })
        .await;
        (url, broadcasts)
    }

    /// A swapped mainnet/testnet endpoint keeps that Arc network served, and
    /// the check names both ids; `/health/ready` reports it `down` with
    /// `rpc_chain_id_mismatch` (`src/readiness.rs`). Nothing is sent by the
    /// check. Through 2.39.1 this was an `Err` that `ProviderCache::from_env`
    /// turned into `exit(1)` for every network, and through 2.39.3 it left Arc
    /// out of /supported until the next deploy.
    #[tokio::test]
    async fn a_swapped_arc_rpc_is_reported_and_arc_stays_served() {
        for (network, expected, wrong_id) in [
            (Network::Arc, 5042, 5042002),
            (Network::ArcTestnet, 5042002, 5042),
        ] {
            let (url, broadcasts) = node_answering(wrong_id).await;
            let provider = EvmProvider::try_new(
                EthereumWallet::from(PrivateKeySigner::random()),
                &url,
                true,
                network,
            )
            .await
            .expect("building a provider asks the RPC nothing");
            assert_eq!(
                crate::chain_identity::check(&provider, std::time::Duration::from_secs(5)).await,
                crate::chain_identity::Verdict::Mismatch {
                    expected,
                    actual: wrong_id
                },
                "{network}"
            );
            let kinds = provider.supported().await.expect("supported").kinds;
            assert!(
                kinds.iter().any(|k| k.network == network.to_string()),
                "{network}: an RPC answering {wrong_id} must not take it out of /supported"
            );
            assert_eq!(broadcasts.load(Ordering::SeqCst), 0);
        }
    }

    /// The whole startup path, as `ProviderCache::from_env` calls it: Arc on a
    /// swapped RPC and Arc on an RPC that does not answer are both built and
    /// served, Base next to them too, and none is an error. Sets and clears
    /// process env, so it relies on the suite's `--test-threads=1`.
    #[tokio::test]
    async fn from_env_serves_arc_whatever_its_rpc_answers_and_the_rest_up() {
        let (arc_url, _) = node_answering(5042002).await;
        let (base_url, _) = node_answering(8453).await;
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let arc_testnet_url = format!("http://{}/", silent.local_addr().unwrap());
        drop(silent);
        let key = format!(
            "0x{}",
            alloy::hex::encode(PrivateKeySigner::random().to_bytes())
        );
        let testnet_key = format!(
            "0x{}",
            alloy::hex::encode(PrivateKeySigner::random().to_bytes())
        );
        let vars = [
            ("SIGNER_TYPE", "private-key".to_string()),
            ("EVM_PRIVATE_KEY_MAINNET", key),
            ("EVM_PRIVATE_KEY_TESTNET", testnet_key),
            ("RPC_URL_ARC", arc_url),
            ("RPC_URL_ARC_TESTNET", arc_testnet_url),
            ("RPC_URL_BASE", base_url),
        ];
        let previous: Vec<_> = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var(name).ok()))
            .collect();
        for (name, value) in &vars {
            std::env::set_var(name, value);
        }

        let arc = EvmProvider::from_env(Network::Arc).await;
        let arc_testnet = EvmProvider::from_env(Network::ArcTestnet).await;
        let base = EvmProvider::from_env(Network::Base).await;

        for (name, value) in previous {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        for (label, built) in [
            ("Arc on a swapped RPC", arc),
            ("Arc testnet on an RPC that does not answer", arc_testnet),
            ("Base", base),
        ] {
            assert!(
                matches!(built, Ok(Some(_))),
                "{label} must be built and served: {:?}",
                built.err()
            );
        }
    }

    #[tokio::test]
    async fn arc_mainnet_stays_disabled_without_its_own_rpc() {
        std::env::remove_var("RPC_URL_ARC");
        assert_eq!(
            from_env::rpc_env_name_from_network(Network::Arc),
            "RPC_URL_ARC"
        );
        assert!(EvmProvider::from_env(Network::Arc).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "Read-only live RPC check, run explicitly before activation"]
    async fn arc_live_rpc_identity_with_the_production_transport() {
        for (network, url) in [
            (Network::Arc, "https://rpc.mainnet.arc.io"),
            (Network::ArcTestnet, "https://rpc.testnet.arc.io"),
        ] {
            let provider = EvmProvider::try_new(
                EthereumWallet::from(PrivateKeySigner::random()),
                url,
                true,
                network,
            )
            .await
            .unwrap_or_else(|e| panic!("{network}: {e}"));
            assert_eq!(
                crate::chain_identity::check(&provider, crate::chain::rpc_http_timeout()).await,
                crate::chain_identity::Verdict::Matches,
                "{network}: {url}"
            );
        }
    }
}

/// Celo Sepolia and HyperEVM testnet, pinned to what their chains answered on
/// 2026-09-23.
///
/// Through 2.39.0 both were built with a chain id that is not theirs -- 44787
/// (Celo Alfajores) and 333 -- and the USDC name `"USD Coin"`. Each contract
/// publishes `name()` = `"USDC"`, `version()` = `"2"`, and a
/// `DOMAIN_SEPARATOR()` that only (`"USDC"`, `"2"`, the real chain id, the
/// address) reproduces.
///
/// What that domain decides here is narrower than it looks. An EOA signature
/// on a non-Arc chain is judged by simulating `transferWithAuthorization`, so
/// the CONTRACT's domain decides it, not ours. Ours is the hash handed to the
/// EIP-6492 validator for a counterfactual wallet and the digest DX402 recovers
/// the payer's key from -- and in both, a wrong domain fails without saying so.
#[cfg(test)]
mod mislabeled_testnet_domain_tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;
    use serde_json::{json, Value};

    const PAYEE: Address = address!("0x2222222222222222222222222222222222222222");
    const NONCE: [u8; 32] = [0x42; 32];

    /// A network whose chain id and USDC name were wrong through 2.39.0.
    struct Case {
        network: Network,
        /// `eth_chainId` of the RPC production uses, read 2026-09-23.
        chain_id: u64,
        /// `DOMAIN_SEPARATOR()` of its USDC, read the same day.
        separator: [u8; 32],
        /// The chain id this facilitator shipped through 2.39.0.
        shipped_chain_id: u64,
        /// The separator of the shipped domain: `"USD Coin"`, `"2"`, that id.
        shipped_separator: [u8; 32],
        /// What the live test reads, unless this env var names another RPC.
        rpc: &'static str,
        rpc_env: &'static str,
    }

    const CASES: [Case; 2] = [
        // forno.celo-sepolia and rpc.ankr.com/celo_sepolia.
        Case {
            network: Network::CeloSepolia,
            chain_id: 11142220,
            separator: hex!("23f491197bb8c5ea4fe8dd4c2293b600f073553f9814015e7a5eb57724df9578"),
            shipped_chain_id: 44787,
            shipped_separator: hex!(
                "9a13e188b45eb3a4e263921a9bc94ebddcb61a0074b8545c1618d5ae5927253d"
            ),
            rpc: "https://forno.celo-sepolia.celo-testnet.org",
            rpc_env: "RPC_URL_CELO_SEPOLIA",
        },
        // rpc.hyperliquid-testnet.xyz/evm and hyperliquid-testnet.drpc.org.
        Case {
            network: Network::HyperEvmTestnet,
            chain_id: 998,
            separator: hex!("f26c39ac3b2040472381fddbd35212755a23586ffb86e608e151af6caa1d465e"),
            shipped_chain_id: 333,
            shipped_separator: hex!(
                "f0e75bc24ee373152600b44d7e91059bc582ba026b4822b27d75b9e01811ea09"
            ),
            rpc: "https://rpc.hyperliquid-testnet.xyz/evm",
            rpc_env: "RPC_URL_HYPEREVM_TESTNET",
        },
    ];

    fn usdc(case: &Case) -> Address {
        USDCDeployment::by_network(case.network)
            .expect("a USDC deployment")
            .address()
            .try_into()
            .expect("an EVM address")
    }

    /// The domain as the facilitator builds it: the static table plus the
    /// chain id, exactly what `assert_domain` and DX402 read.
    fn our_domain(case: &Case) -> Eip712Domain {
        let (name, version) = find_known_eip712_metadata(case.network, &usdc(case))
            .expect("the USDC is in the static EIP-712 table");
        eip712_domain! {
            name: name,
            version: version,
            chain_id: EvmChain::try_from(case.network).unwrap().chain_id,
            verifying_contract: usdc(case),
        }
    }

    /// The domain the contract publishes, written out by a client that read it
    /// from the chain -- not from our table, or the vector would compare the
    /// table with itself.
    fn live_domain(case: &Case) -> Eip712Domain {
        eip712_domain! {
            name: "USDC",
            version: "2",
            chain_id: case.chain_id,
            verifying_contract: usdc(case),
        }
    }

    /// The domain a client got by trusting `/supported` through 2.39.0.
    fn shipped_domain(case: &Case) -> Eip712Domain {
        eip712_domain! {
            name: "USD Coin",
            version: "2",
            chain_id: case.shipped_chain_id,
            verifying_contract: usdc(case),
        }
    }

    /// A `/verify` body on `case`'s network for `value`, signed by `payer`
    /// under `domain`.
    fn body(case: &Case, payer: &PrivateKeySigner, value: u64, domain: &Eip712Domain) -> Value {
        let valid_before = crate::erc8004::proof::unix_now_secs() + 300;
        let transfer = TransferWithAuthorization {
            from: payer.address(),
            to: PAYEE,
            value: U256::from(value),
            validAfter: U256::ZERO,
            validBefore: U256::from(valid_before),
            nonce: FixedBytes(NONCE),
        };
        let signature = payer
            .sign_hash_sync(&transfer.eip712_signing_hash(domain))
            .expect("signs");
        let network = case.network.to_string();
        json!({
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": network,
                "payload": {
                    "signature": format!("0x{}", hex::encode(signature.as_bytes())),
                    "authorization": {
                        "from": payer.address(),
                        "to": PAYEE,
                        "value": value.to_string(),
                        "validAfter": "0",
                        "validBefore": valid_before.to_string(),
                        "nonce": format!("0x{}", hex::encode(NONCE)),
                    }
                }
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": network,
                "maxAmountRequired": value.to_string(),
                "resource": "https://example.com/paid",
                "description": "",
                "mimeType": "application/json",
                "payTo": PAYEE,
                "maxTimeoutSeconds": 300,
                "asset": usdc(case),
                "extra": {"name": "USDC", "version": "2"},
            }
        })
    }

    /// The payment `assert_valid_payment` would hand on, built from the wire.
    fn payment(case: &Case, request: &VerifyRequest) -> ExactEvmPayment {
        let ExactPaymentPayload::Evm(evm) = &request.payment_payload.payload else {
            panic!("an EVM payload");
        };
        ExactEvmPayment {
            chain: EvmChain::try_from(case.network).unwrap(),
            from: evm.authorization.from,
            to: evm.authorization.to,
            value: evm.authorization.value,
            valid_after: evm.authorization.valid_after,
            valid_before: evm.authorization.valid_before,
            nonce: evm.authorization.nonce,
            signature: evm.signature.clone(),
        }
    }

    #[test]
    fn each_network_is_the_chain_its_rpc_answers_for() {
        for case in &CASES {
            assert_eq!(
                EvmChain::try_from(case.network).unwrap().chain_id,
                case.chain_id,
                "{}",
                case.network
            );
        }
    }

    /// The four fields a signature commits to, checked against the one number
    /// each contract publishes -- and the shipped domain checked against the
    /// same number, so the assertion cannot be met by a chain id that is
    /// ignored.
    #[test]
    fn our_domain_is_the_one_the_contract_publishes() {
        for case in &CASES {
            let network = case.network;
            assert_eq!(our_domain(case).separator().0, case.separator, "{network}");
            assert_eq!(live_domain(case).separator().0, case.separator, "{network}");
            assert_eq!(
                shipped_domain(case).separator().0,
                case.shipped_separator,
                "{network}"
            );
            assert_ne!(case.shipped_separator, case.separator, "{network}");
        }
    }

    /// The vector. One payer signs the same authorization twice: under the
    /// domain the contract publishes (`"USDC"`, the real chain id) and under
    /// the one this facilitator shipped (`"USD Coin"`, the wrong id). Hashed
    /// the way the facilitator hashes it -- `SignedMessage::extract` under our
    /// domain -- the first recovers its payer and the second recovers somebody
    /// else.
    #[test]
    fn a_signature_for_the_live_domain_verifies_and_one_for_the_shipped_domain_does_not() {
        let payer = PrivateKeySigner::random();
        for case in &CASES {
            for (domain, label, verifies) in [
                (live_domain(case), "live", true),
                (shipped_domain(case), "shipped", false),
            ] {
                let request: VerifyRequest =
                    serde_json::from_value(body(case, &payer, 10_000, &domain)).expect("parses");
                let payment = payment(case, &request);
                let signed = SignedMessage::extract(&payment, &our_domain(case)).expect("extracts");
                let recovered =
                    alloy::primitives::Signature::try_from(payment.signature.0.as_slice())
                        .expect("65 bytes")
                        .recover_address_from_prehash(&signed.hash)
                        .expect("recovers some address");
                assert_eq!(
                    recovered == payer.address(),
                    verifies,
                    "{}: a signature under the {label} domain must {}verify under ours",
                    case.network,
                    if verifies { "" } else { "not " }
                );
            }
        }
    }

    /// Every EVM chain id the facilitator signs for is the one its CAIP-2 id
    /// names. The two are separate tables; celo-sepolia and hyperevm-testnet
    /// were consistent across both and still wrong, which no test can see
    /// without the chain -- but a table that drifts from the other is caught
    /// here.
    #[test]
    fn every_evm_chain_id_is_the_one_its_caip2_names() {
        for &network in Network::variants() {
            let Ok(chain) = EvmChain::try_from(network) else {
                continue;
            };
            assert_eq!(
                network.to_caip2(),
                format!("eip155:{}", chain.chain_id),
                "{network}: EvmChain and to_caip2 disagree"
            );
        }
    }

    /// Every network of the EVM family declares a chain id. `EvmChain::try_from`
    /// is an exhaustive match, but an arm can still answer `UnsupportedNetwork`
    /// for an EVM variant, and the test above skips those: a new EVM network
    /// entered that way would have no id for its EIP-712 domain, and nothing for
    /// the startup check (`chain_identity`) to compare its RPC with.
    #[test]
    fn every_evm_network_declares_a_chain_id() {
        for &network in Network::variants() {
            if matches!(
                crate::network::NetworkFamily::from(network),
                crate::network::NetworkFamily::Evm
            ) {
                assert!(
                    EvmChain::try_from(network).is_ok(),
                    "{network} is an EVM network and declares no chain id in EvmChain::try_from"
                );
            }
        }
    }

    /// The facilitator's own `verify`, against each chain itself. Read-only:
    /// `verify` simulates with `eth_call` and never broadcasts. Value 0, so a
    /// fresh random payer with no balance reaches the signature check.
    #[tokio::test]
    #[ignore = "Read-only live RPC check against the testnets; run explicitly"]
    async fn live_verify_accepts_only_the_contracts_domain() {
        for case in &CASES {
            let network = case.network;
            let url = std::env::var(case.rpc_env).unwrap_or_else(|_| case.rpc.into());
            let provider = EvmProvider::try_new(
                EthereumWallet::from(PrivateKeySigner::random()),
                &url,
                true,
                network,
            )
            .await
            .expect("provider");
            assert_eq!(
                provider.inner().get_chain_id().await.expect("eth_chainId"),
                case.chain_id,
                "{url} is not {network}"
            );
            let payer = PrivateKeySigner::random();

            let live: VerifyRequest =
                serde_json::from_value(body(case, &payer, 0, &live_domain(case))).unwrap();
            match provider.verify(&live).await {
                Ok(VerifyResponse::Valid { payer: p }) => {
                    assert_eq!(p, MixedAddress::from(payer.address()))
                }
                other => panic!("{network}: the live domain must verify: {other:?}"),
            }

            let shipped: VerifyRequest =
                serde_json::from_value(body(case, &payer, 0, &shipped_domain(case))).unwrap();
            match provider.verify(&shipped).await {
                Ok(VerifyResponse::Valid { .. }) => {
                    panic!("{network}: the shipped domain must not verify")
                }
                refused => eprintln!("{network}: shipped domain refused as expected: {refused:?}"),
            }
        }
    }
}

/// The ERC-8004 proof of payment a settle emits, end to end.
///
/// A proof is only worth emitting if `verify_payment_facts` -- the check behind
/// every rating that carries one -- accepts it, and it is optional: producing it
/// may never cost the settle its answer, nor hold it open without a bound. Until
/// 2.29.6 the proof carried the facilitator's clock while the verifier requires
/// the block's timestamp to the second, so the facilitator's own proofs failed
/// with `proof_timestamp_mismatch`.
///
/// Each test drives the real `Facilitator::settle` of an `EvmProvider` against a
/// local JSON-RPC mock and hands the proof to the verifier over the same mock
/// chain. The chain height is frozen below the payment's block, so the receipt
/// watcher's heartbeat never asks for that block: every read of it the mock
/// counts belongs to the proof.
#[cfg(test)]
mod proof_of_payment_tests {
    use super::*;
    use crate::erc8004::proof::{unix_now_secs, verify_payment_facts, ProofRejection};
    use alloy::primitives::keccak256;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;
    use axum::{extract::State, routing::post, Json as AxumJson, Router};
    use serde_json::{json, Value};
    use std::time::Duration;

    /// The block the mock mines the payment in.
    const BLOCK: u64 = 0x10_0000;
    /// What `eth_blockNumber` answers: below [`BLOCK`], and never moving.
    const TIP: u64 = BLOCK - 10;
    /// The hash `eth_sendRawTransaction` hands back, and the one the receipt
    /// carries.
    const TX: [u8; 32] = [0x5e; 32];
    const BLOCK_HASH: [u8; 32] = [0xbb; 32];
    const PAYEE: Address = address!("0x2222222222222222222222222222222222222222");
    const AMOUNT: u64 = 1_000_000;
    /// The payment is seconds old; the window only has to cover that.
    const MAX_AGE_SECS: u64 = 900;

    /// How the node answers `eth_getBlockByNumber` for [`BLOCK`].
    #[derive(Clone, Copy)]
    enum BlockRead {
        Serve,
        /// A JSON-RPC error.
        Fail,
        /// `null`, as a load-balanced node behind the one that served the
        /// receipt answers.
        Missing,
        /// No answer for a minute.
        Hang,
    }

    #[derive(Clone)]
    struct MockChain {
        payer: Address,
        token: Address,
        /// Thirty seconds in the past, so it cannot coincide with the
        /// facilitator's clock.
        block_timestamp: u64,
        /// Whether the receipt's logs carry `blockTimestamp`, as most public
        /// nodes do and Avalanche's does not.
        logs_carry_timestamp: bool,
        block_read: BlockRead,
        /// Reads of [`BLOCK`], answered or not.
        block_reads: Arc<AtomicUsize>,
    }

    fn base_usdc() -> Address {
        USDCDeployment::by_network(Network::Base)
            .expect("Base has a USDC deployment")
            .address()
            .try_into()
            .expect("Base USDC is an EVM address")
    }

    /// `eth_getTransactionReceipt` for the payment: one `Transfer` of [`AMOUNT`]
    /// from `payer` to [`PAYEE`] in `token`, mined in `block`.
    fn receipt_json(
        payer: Address,
        token: Address,
        block: Option<u64>,
        log_timestamp: Option<u64>,
    ) -> Value {
        let block_number = block.map(|n| format!("{n:#x}"));
        let mut log = json!({
            "address": token,
            "topics": [
                keccak256("Transfer(address,address,uint256)"),
                payer.into_word(),
                PAYEE.into_word(),
            ],
            "data": format!("0x{}", hex::encode(U256::from(AMOUNT).to_be_bytes::<32>())),
            "blockNumber": block_number,
            "blockHash": format!("0x{}", hex::encode(BLOCK_HASH)),
            "transactionHash": format!("0x{}", hex::encode(TX)),
            "transactionIndex": "0x0",
            "logIndex": "0x0",
            "removed": false
        });
        if let Some(ts) = log_timestamp {
            log["blockTimestamp"] = json!(format!("{ts:#x}"));
        }
        json!({
            "transactionHash": format!("0x{}", hex::encode(TX)),
            "transactionIndex": "0x0",
            "blockHash": format!("0x{}", hex::encode(BLOCK_HASH)),
            "blockNumber": block_number,
            "from": address!("0x0000000000000000000000000000000000000001"),
            "to": token,
            "cumulativeGasUsed": "0x5208",
            "gasUsed": "0x5208",
            "contractAddress": null,
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "status": "0x1",
            "type": "0x0",
            "effectiveGasPrice": "0x3b9aca00",
            "logs": [log]
        })
    }

    /// `eth_getBlockByNumber`, trimmed to what alloy needs to deserialise.
    fn block_json(number: u64, timestamp: u64) -> Value {
        let zero32 = format!("0x{}", hex::encode([0u8; 32]));
        json!({
            "hash": format!("0x{}", hex::encode(BLOCK_HASH)),
            "parentHash": zero32,
            "sha3Uncles": zero32,
            "miner": format!("0x{}", hex::encode([0u8; 20])),
            "stateRoot": zero32,
            "transactionsRoot": zero32,
            "receiptsRoot": zero32,
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "difficulty": "0x0",
            "number": format!("{number:#x}"),
            "gasLimit": "0x1c9c380",
            "gasUsed": "0x5208",
            "timestamp": format!("{timestamp:#x}"),
            "extraData": "0x",
            "mixHash": zero32,
            "nonce": "0x0000000000000000",
            "baseFeePerGas": "0x1",
            "totalDifficulty": "0x0",
            "size": "0x220",
            "transactions": [],
            "uncles": []
        })
    }

    async fn answer(chain: &MockChain, req: &Value) -> Value {
        let id = req.get("id").cloned().unwrap_or(json!(1));
        let result = match req["method"].as_str().unwrap_or_default() {
            "eth_chainId" => json!("0x2105"),
            "eth_getTransactionCount" => json!("0x0"),
            "eth_gasPrice" => json!("0x3b9aca00"),
            "eth_estimateGas" => json!("0x5208"),
            // `balanceOf`, the only contract read a settle makes.
            "eth_call" => json!(format!(
                "0x{}",
                hex::encode(U256::from(AMOUNT * 10).to_be_bytes::<32>())
            )),
            "eth_sendRawTransaction" => json!(format!("0x{}", hex::encode(TX))),
            "eth_blockNumber" => json!(format!("{TIP:#x}")),
            "eth_getTransactionReceipt" => receipt_json(
                chain.payer,
                chain.token,
                Some(BLOCK),
                chain.logs_carry_timestamp.then_some(chain.block_timestamp),
            ),
            "eth_getBlockByNumber" => {
                let number = req["params"][0]
                    .as_str()
                    .and_then(|n| u64::from_str_radix(n.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_default();
                if number != BLOCK {
                    // The heartbeat, reading at the tip.
                    block_json(number, chain.block_timestamp)
                } else {
                    chain.block_reads.fetch_add(1, Ordering::SeqCst);
                    match chain.block_read {
                        BlockRead::Serve => block_json(BLOCK, chain.block_timestamp),
                        BlockRead::Missing => Value::Null,
                        BlockRead::Fail => {
                            return json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": {"code": -32603, "message": "internal error"},
                            });
                        }
                        BlockRead::Hang => {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            block_json(BLOCK, chain.block_timestamp)
                        }
                    }
                }
            }
            _ => Value::Null,
        };
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    async fn rpc(
        State(chain): State<MockChain>,
        AxumJson(body): AxumJson<Value>,
    ) -> AxumJson<Value> {
        AxumJson(match &body {
            Value::Array(reqs) => {
                let mut out = Vec::with_capacity(reqs.len());
                for req in reqs {
                    out.push(answer(&chain, req).await);
                }
                Value::Array(out)
            }
            req => answer(&chain, req).await,
        })
    }

    async fn spawn_rpc(chain: MockChain) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/", post(rpc)).with_state(chain);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/")
    }

    /// A settle of [`AMOUNT`] to [`PAYEE`] in `token`, authorised by `payer`,
    /// carrying the `8004-reputation` extension when `wants_proof`.
    fn settle_request(
        payer: &PrivateKeySigner,
        token: Address,
        wants_proof: bool,
    ) -> SettleRequest {
        let valid_before = unix_now_secs() + 600;
        let nonce = [0x42u8; 32];
        let (name, version) = find_known_eip712_metadata(Network::Base, &token)
            .expect("Base USDC is in the static EIP-712 table");
        let domain = eip712_domain! {
            name: name,
            version: version,
            chain_id: 8453,
            verifying_contract: token,
        };
        let authorization = TransferWithAuthorization {
            from: payer.address(),
            to: PAYEE,
            value: U256::from(AMOUNT),
            validAfter: U256::ZERO,
            validBefore: U256::from(valid_before),
            nonce: FixedBytes(nonce),
        };
        let signature = payer
            .sign_hash_sync(&authorization.eip712_signing_hash(&domain))
            .expect("signs");
        let mut extra = serde_json::Map::new();
        if wants_proof {
            extra.insert(crate::erc8004::EXTENSION_ID.to_string(), json!({}));
        }
        serde_json::from_value(json!({
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": "base",
                "payload": {
                    "signature": format!("0x{}", hex::encode(signature.as_bytes())),
                    "authorization": {
                        "from": payer.address(),
                        "to": PAYEE,
                        "value": AMOUNT.to_string(),
                        "validAfter": "0",
                        "validBefore": valid_before.to_string(),
                        "nonce": format!("0x{}", hex::encode(nonce)),
                    }
                }
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": "base",
                "maxAmountRequired": AMOUNT.to_string(),
                "resource": "https://example.com/paid",
                "description": "",
                "mimeType": "application/json",
                "payTo": PAYEE,
                "maxTimeoutSeconds": 60,
                "asset": token,
                "extra": extra,
            }
        }))
        .expect("settle request parses")
    }

    struct Settled {
        response: Result<SettleResponse, FacilitatorLocalError>,
        elapsed: Duration,
        /// Reads of [`BLOCK`] made by the settle.
        block_reads: usize,
        block_timestamp: u64,
        /// The mock, still serving, for the verifier.
        url: String,
    }

    async fn settle(
        block_read: BlockRead,
        logs_carry_timestamp: bool,
        wants_proof: bool,
    ) -> Settled {
        let payer = PrivateKeySigner::random();
        let chain = MockChain {
            payer: payer.address(),
            token: base_usdc(),
            block_timestamp: unix_now_secs() - 30,
            logs_carry_timestamp,
            block_read,
            block_reads: Arc::default(),
        };
        let url = spawn_rpc(chain.clone()).await;
        // `eip1559 = false`: pricing is one `eth_gasPrice`, not a fee history.
        let facilitator = EvmProvider::try_new(
            EthereumWallet::from(PrivateKeySigner::random()),
            &url,
            false,
            Network::Base,
        )
        .await
        .expect("provider");
        let request = settle_request(&payer, chain.token, wants_proof);

        let started = std::time::Instant::now();
        let response = facilitator.settle(&request).await;
        Settled {
            response,
            elapsed: started.elapsed(),
            block_reads: chain.block_reads.load(Ordering::SeqCst),
            block_timestamp: chain.block_timestamp,
            url,
        }
    }

    /// The response of a settle whose transfer the chain confirmed, which has
    /// to be a success whatever became of the proof.
    fn confirmed(settled: &Settled) -> &SettleResponse {
        let response = settled.response.as_ref().unwrap_or_else(|e| {
            panic!("the transfer is confirmed on chain, so the settle must succeed; got {e:?}")
        });
        assert!(response.success, "confirmed transfer reported as failed");
        assert!(
            matches!(response.transaction, Some(TransactionHash::Evm(tx)) if tx == TX),
            "the response lost the transaction hash: {:?}",
            response.transaction,
        );
        response
    }

    async fn verify(url: &str, proof: &ProofOfPayment) -> Result<(), ProofRejection> {
        let rpc = ProviderBuilder::new().connect_http(url.parse().expect("mock url"));
        verify_payment_facts(&rpc, Network::Base, proof, MAX_AGE_SECS)
            .await
            .map(|_| ())
    }

    /// The round trip the defect broke. The receipt's logs carry no
    /// `blockTimestamp`, so the proof needs the block itself.
    #[tokio::test]
    async fn the_proof_a_settle_emits_passes_the_proof_verifier() {
        let settled = settle(BlockRead::Serve, false, true).await;
        let proof = confirmed(&settled)
            .proof_of_payment
            .clone()
            .expect("the settle asked for a proof and the chain answered every read");

        assert_eq!(
            verify(&settled.url, &proof).await,
            Ok(()),
            "the facilitator's own proof must pass the verifier behind /feedback",
        );
        assert_eq!(proof.block_number, BLOCK);
        assert_eq!(proof.timestamp, settled.block_timestamp);
        assert_eq!(settled.block_reads, 1, "one block read per proof");
    }

    /// Most nodes put `blockTimestamp` on every log of a receipt, and then the
    /// block is not read at all.
    #[tokio::test]
    async fn a_receipt_whose_logs_carry_the_block_timestamp_costs_no_block_read() {
        let settled = settle(BlockRead::Serve, true, true).await;
        let proof = confirmed(&settled)
            .proof_of_payment
            .clone()
            .expect("the receipt carried everything the proof needs");

        assert_eq!(verify(&settled.url, &proof).await, Ok(()));
        assert_eq!(
            settled.block_reads, 0,
            "the receipt already had the timestamp"
        );
    }

    #[tokio::test]
    async fn a_settle_that_asks_for_no_proof_reads_no_block() {
        let settled = settle(BlockRead::Serve, false, false).await;
        assert!(confirmed(&settled).proof_of_payment.is_none());
        assert_eq!(settled.block_reads, 0);
    }

    /// A proof the verifier is bound to reject is worse than none: without one
    /// a rating takes the provisional path instead of carrying a failure.
    #[tokio::test]
    async fn a_block_read_that_fails_settles_without_a_proof() {
        let settled = settle(BlockRead::Fail, false, true).await;
        assert!(confirmed(&settled).proof_of_payment.is_none());
        assert_eq!(settled.block_reads, 1, "a failed read is not retried");
    }

    #[tokio::test]
    async fn a_node_without_the_block_settles_without_a_proof() {
        let settled = settle(BlockRead::Missing, false, true).await;
        assert!(confirmed(&settled).proof_of_payment.is_none());
        assert_eq!(settled.block_reads, 1, "a missing block is not polled for");
    }

    /// No proof, rather than one naming block 0. The node would serve any
    /// block, so only the missing number can stop the proof.
    #[tokio::test]
    async fn a_receipt_without_a_block_number_yields_no_proof() {
        let payer = PrivateKeySigner::random();
        let token = base_usdc();
        let receipt: TransactionReceipt =
            serde_json::from_value(receipt_json(payer.address(), token, None, None))
                .expect("receipt parses");
        let request = settle_request(&payer, token, true);
        let node = alloy::providers::mock::Asserter::new();
        node.push_success(&block_json(0, unix_now_secs() - 30));
        let rpc = ProviderBuilder::new().connect_mocked_client(node);

        let proof = create_proof_of_payment(
            &rpc,
            &receipt,
            &request.payment_requirements,
            Network::Base,
            MixedAddress::Evm(payer.address().into()),
            MixedAddress::Evm(PAYEE.into()),
            TokenAmount::from(U256::from(AMOUNT)),
            MixedAddress::Evm(token.into()),
        )
        .await;

        assert!(
            proof.is_none(),
            "got a proof for a receipt with no block: {proof:?}"
        );
    }

    /// `elapsed >= bound` is asserted so that a read failing instantly cannot
    /// pass for the timeout.
    #[tokio::test]
    async fn a_block_read_that_hangs_holds_the_settle_no_longer_than_its_bound() {
        let settled = settle(BlockRead::Hang, false, true).await;
        assert!(confirmed(&settled).proof_of_payment.is_none());
        assert!(
            settled.elapsed >= PROOF_BLOCK_READ_TIMEOUT,
            "returned in {:?}, before the bound: the timeout was never exercised",
            settled.elapsed,
        );
        assert!(
            settled.elapsed < PROOF_BLOCK_READ_TIMEOUT + Duration::from_secs(5),
            "the settle waited {:?} on a proof it does not need",
            settled.elapsed,
        );
    }
}
