use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{
    RpcSendTransactionConfig, RpcSimulateTransactionAccountsConfig, RpcSimulateTransactionConfig,
};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::clock::Clock;
use solana_sdk::instruction::CompiledInstruction;
use solana_sdk::pubkey;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status_client_types::{
    option_serializer::OptionSerializer, UiInnerInstructions, UiInstruction,
    UiTransactionStatusMeta,
};
use spl_token_2022::extension::transfer_fee::TransferFeeConfig;
use spl_token_2022::extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing_core::Level;

use once_cell::sync::OnceCell;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::Network;
use crate::nonce_store::NonceStore;
use crate::types::{
    Base64Bytes, ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, PaymentRequirements,
    SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
    SupportedPaymentKindsResponse, TokenAmount, TransactionHash, VerifyRequest, VerifyResponse,
};
use crate::types::{Scheme, X402Version};

const ATA_PROGRAM_PUBKEY: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// SPL Token account layout: mint[0..32] | owner[32..64] | amount[64..72] | ...
// Parse the u64 amount at byte offset 64 from raw token account data.
fn parse_token_account_balance(raw: &[u8]) -> Option<u64> {
    if raw.len() < 72 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&raw[64..72]);
    Some(u64::from_le_bytes(buf))
}

/// Upper bound on the transfer fee a mint takes from a transfer of `amount`:
/// the larger of the fee in force at `epoch` and the newer fee in the config.
///
/// The newer fee counts even while it is still scheduled. spl-token-2022 starts a
/// new fee two epochs after it is set (`newer_fee_start_epoch = epoch + 2` in the
/// transfer_fee processor), so reading both entries means a fee change can never
/// land between a verify that saw 0 and the settlement it approved.
///
/// A legacy SPL Token mint has no extensions and charges nothing. An account that
/// no token program owns is refused rather than read as fee-free.
fn transfer_fee_upper_bound(
    mint_owner: &Pubkey,
    mint_data: &[u8],
    epoch: u64,
    amount: u64,
) -> Result<u64, FacilitatorLocalError> {
    if *mint_owner == spl_token::ID {
        return Ok(0);
    }
    if *mint_owner != spl_token_2022::ID {
        return Err(FacilitatorLocalError::DecodingError(
            "invalid_exact_svm_payload_transaction_mint_not_a_token_mint".to_string(),
        ));
    }
    let unreadable = || {
        FacilitatorLocalError::DecodingError(
            "invalid_exact_svm_payload_transaction_mint_unreadable".to_string(),
        )
    };
    let mint = StateWithExtensions::<spl_token_2022::state::Mint>::unpack(mint_data)
        .map_err(|_| unreadable())?;
    let has_fee_config = mint
        .get_extension_types()
        .map_err(|_| unreadable())?
        .contains(&ExtensionType::TransferFeeConfig);
    if !has_fee_config {
        return Ok(0);
    }
    let config = mint
        .get_extension::<TransferFeeConfig>()
        .map_err(|_| unreadable())?;
    let in_force = config.calculate_epoch_fee(epoch, amount);
    let newer = config.newer_transfer_fee.calculate_fee(amount);
    match (in_force, newer) {
        (Some(in_force), Some(newer)) => Ok(in_force.max(newer)),
        _ => Err(unreadable()),
    }
}

// ============================================================================
// Replay protection for settlement-account path (audit fix B1)
// ============================================================================
//
// Solana transaction signatures are globally unique and permanent once confirmed.
// We store each settled tx_signature so the same on-chain tx cannot be double-settled.
// Key format: `solana-settle#{network}#{tx_signature_base58}`
// TTL: 7 days (well beyond any reasonable re-processing window).
//
// SECURITY MODEL NOTE: The settleSecretKey field in SettlementAccountPayload transmits
// a private key over the wire. This is the fundamental residual risk of the settlement
// account design. Even with the B1 fixes applied, a compromised transport layer could
// expose the settlement account keypair. Recommended next step: migrate to a PDA model
// where the facilitator controls a deterministic PDA for each payer/merchant pair, and
// the client only needs to fund that PDA. This eliminates the need to transmit any
// private key material. See docs/reports/2026-05-19-security-audit.md finding B1.

static SOLANA_NONCE_STORE: OnceCell<std::sync::Arc<dyn NonceStore>> = OnceCell::new();

async fn get_solana_nonce_store() -> std::sync::Arc<dyn NonceStore> {
    if let Some(store) = SOLANA_NONCE_STORE.get() {
        return store.clone();
    }
    let store = crate::nonce_store::create_nonce_store().await;
    let _ = SOLANA_NONCE_STORE.set(store.clone());
    store
}

fn solana_settle_nonce_key(network: &str, tx_signature: &str) -> String {
    format!("solana-settle#{}#{}", network, tx_signature)
}

/// Kill-switch for the settlement-account (Crossmint custodial) scheme (audit 03).
/// The forgery vector is already closed by the pay_to-credit binding in
/// `verify_settlement_account` and the empty-ATA hard-error in `sweep_settlement_account`,
/// so this gate is defense-in-depth: it defaults ON to preserve existing Crossmint flows, and
/// can be set ENABLE_SETTLEMENT_ACCOUNT=false to disable the niche path entirely if unused.
pub fn is_settlement_account_enabled() -> bool {
    std::env::var("ENABLE_SETTLEMENT_ACCOUNT")
        .map(|v| !(v.eq_ignore_ascii_case("false") || v == "0"))
        .unwrap_or(true)
}

/// The settlement keypair a `settleSecretKey` carries (base58, 64 bytes).
///
/// Verify and sweep both decode it here, so the account verify binds the
/// deposit to is the account the sweep signs for. `Keypair::from_bytes`
/// refuses bytes whose public half is not the one their secret half derives.
fn settlement_keypair(secret_key_str: &str) -> Result<Keypair, FacilitatorLocalError> {
    let secret_bytes = solana_sdk::bs58::decode(secret_key_str)
        .into_vec()
        .map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("invalid settleSecretKey: {e}"))
        })?;
    Keypair::from_bytes(&secret_bytes).map_err(|e| {
        FacilitatorLocalError::DecodingError(format!("invalid settleSecretKey keypair: {e}"))
    })
}

// TTL for settlement replay records: 7 days in seconds.
const SOLANA_SETTLE_TTL_SECONDS: u64 = 7 * 24 * 3600;

/// The `errorReason` of a settle whose transaction confirmed without paying
/// payTo the price: the payment did not happen.
pub const LANDED_WITHOUT_PAYMENT: &str = "settlement_landed_without_payment";

/// How many times settle asks for a confirmed transaction, and how far apart,
/// before it gives up on reading what the transaction did. A node behind the
/// one that confirmed it may not hand it back at once.
const LANDED_READ_ATTEMPTS: u32 = 8;
const LANDED_READ_INTERVAL: Duration = Duration::from_millis(750);

/// What a confirmed transaction did for payTo, as the chain recorded it.
#[derive(Debug, PartialEq, Eq)]
enum LandedPayment {
    /// payTo's holdings of the asset grew by the price or more.
    Paid,
    /// The transaction failed on chain, or left payTo short of the price.
    NotPaid(String),
    /// No verdict: the transaction, or what it did for payTo, could not be read.
    Unknown(String),
}

/// What a landed transaction's status and token balances say it paid payTo.
///
/// The balances are the transaction's own, recorded before and after it ran,
/// so nothing else paid to payTo counts. payTo's share is the net change over
/// every account of `mint` that payTo owns: a credit that another of its
/// accounts gives back is no payment. A balance of `mint` that names no owner
/// leaves that share unknown.
fn landed_payment(
    meta: &UiTransactionStatusMeta,
    pay_to: &Pubkey,
    mint: &Pubkey,
    required: u64,
) -> LandedPayment {
    if let Some(err) = &meta.err {
        return LandedPayment::NotPaid(format!("failed on chain: {err}"));
    }
    let (OptionSerializer::Some(before), OptionSerializer::Some(after)) =
        (&meta.pre_token_balances, &meta.post_token_balances)
    else {
        return LandedPayment::Unknown("no token balances recorded".to_string());
    };
    let mint = mint.to_string();
    let mut received: i128 = 0;
    for (sign, balances) in [(-1i128, before), (1, after)] {
        for balance in balances.iter().filter(|balance| balance.mint == mint) {
            let OptionSerializer::Some(owner) = &balance.owner else {
                return LandedPayment::Unknown(format!(
                    "the balance of account {} names no owner",
                    balance.account_index
                ));
            };
            match Pubkey::from_str(owner) {
                Ok(owner) if owner == *pay_to => {}
                Ok(_) => continue,
                Err(_) => {
                    return LandedPayment::Unknown(format!(
                        "the balance of account {} names an unreadable owner",
                        balance.account_index
                    ))
                }
            }
            let Ok(amount) = balance.ui_token_amount.amount.parse::<u64>() else {
                return LandedPayment::Unknown(format!(
                    "the balance of account {} is unreadable",
                    balance.account_index
                ));
            };
            received += sign * i128::from(amount);
        }
    }
    if received >= i128::from(required) {
        LandedPayment::Paid
    } else {
        LandedPayment::NotPaid(format!("payTo received {received} of {required}"))
    }
}

#[derive(Clone, Debug)]
pub struct SolanaChain {
    pub network: Network,
}

impl TryFrom<Network> for SolanaChain {
    type Error = FacilitatorLocalError;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Solana => Ok(Self { network: value }),
            Network::SolanaDevnet => Ok(Self { network: value }),
            Network::BaseSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Base => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::XdcMainnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::AvalancheFuji => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Avalanche => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::PolygonAmoy => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Polygon => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Sei => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::SeiTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            // Custom networks added by Ultravioleta DAO
            Network::Optimism => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::OptimismSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Celo => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::CeloSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::HyperEvm => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::HyperEvmTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Ethereum => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::EthereumSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Arbitrum => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::ArbitrumSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Unichain => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::UnichainSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Monad => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Bsc => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::SkaleBase => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::SkaleBaseSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Scroll => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Robinhood => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::RobinhoodTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Arc | Network::ArcTestnet => {
                Err(FacilitatorLocalError::UnsupportedNetwork(None))
            }
            Network::HashKey => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::HashKeyTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
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
            Network::Fogo => Ok(Self { network: value }),
            Network::FogoTestnet => Ok(Self { network: value }),
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

#[derive(Clone, Debug)]
pub struct SolanaAddress {
    pubkey: Pubkey,
}

impl From<Pubkey> for SolanaAddress {
    fn from(pubkey: Pubkey) -> Self {
        Self { pubkey }
    }
}

impl From<SolanaAddress> for Pubkey {
    fn from(address: SolanaAddress) -> Self {
        address.pubkey
    }
}

impl TryFrom<MixedAddress> for SolanaAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: MixedAddress) -> Result<Self, Self::Error> {
        match value {
            MixedAddress::Evm(_)
            | MixedAddress::Offchain(_)
            | MixedAddress::Near(_)
            | MixedAddress::Stellar(_)
            | MixedAddress::Algorand(_) => Err(FacilitatorLocalError::InvalidAddress(
                "expected Solana address".to_string(),
            )),
            #[cfg(feature = "sui")]
            MixedAddress::Sui(_) => Err(FacilitatorLocalError::InvalidAddress(
                "expected Solana address".to_string(),
            )),
            #[cfg(feature = "xrpl")]
            MixedAddress::Xrpl(_) => Err(FacilitatorLocalError::InvalidAddress(
                "expected Solana address".to_string(),
            )),
            #[cfg(feature = "hedera")]
            MixedAddress::Hedera(_) => Err(FacilitatorLocalError::InvalidAddress("expected Solana address".into())),
            MixedAddress::Solana(pubkey) => Ok(Self { pubkey }),
        }
    }
}

impl From<SolanaAddress> for MixedAddress {
    fn from(value: SolanaAddress) -> Self {
        MixedAddress::Solana(value.pubkey)
    }
}

#[derive(Clone)]
pub struct SolanaProvider {
    keypair: Arc<Keypair>,
    chain: SolanaChain,
    rpc_client: Arc<RpcClient>,
    max_compute_unit_limit: u32,
    max_compute_unit_price: u64,
}

impl Debug for SolanaProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SolanaProvider")
            .field("pubkey", &self.keypair.pubkey())
            .field("chain", &self.chain)
            .field("rpc_url", &self.rpc_client.url())
            .finish()
    }
}

impl SolanaProvider {
    fn max_compute_unit_limit_from_env(network: Network) -> u32 {
        let suffix = match network {
            Network::Solana => "SOLANA",
            Network::SolanaDevnet => "SOLANA_DEVNET",
            Network::Fogo => "FOGO",
            Network::FogoTestnet => "FOGO_TESTNET",
            _ => return 200_000, // fallback (shouldn't be used)
        };

        let limit_var = format!("X402_SOLANA_MAX_COMPUTE_UNIT_LIMIT_{}", suffix);
        std::env::var(&limit_var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(match network {
                Network::Solana => 400_000,
                Network::SolanaDevnet => 200_000,
                Network::Fogo => 400_000,
                Network::FogoTestnet => 200_000,
                _ => 200_000,
            })
    }

    fn max_compute_unit_price_from_env(network: Network) -> u64 {
        let suffix = match network {
            Network::Solana => "SOLANA",
            Network::SolanaDevnet => "SOLANA_DEVNET",
            Network::Fogo => "FOGO",
            Network::FogoTestnet => "FOGO_TESTNET",
            _ => return 100_000, // fallback (shouldn't be used)
        };

        let price_var = format!("X402_SOLANA_MAX_COMPUTE_UNIT_PRICE_{}", suffix);
        std::env::var(&price_var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(match network {
                Network::Solana => 1_000_000,
                Network::SolanaDevnet => 100_000,
                Network::Fogo => 1_000_000,
                Network::FogoTestnet => 100_000,
                _ => 100_000,
            })
    }

    pub fn try_new(
        keypair: Keypair,
        rpc_url: String,
        network: Network,
        max_compute_unit_limit: u32,
        max_compute_unit_price: u64,
    ) -> Result<Self, FacilitatorLocalError> {
        let chain = SolanaChain::try_from(network)?;
        {
            let signer_addresses = vec![keypair.pubkey()];
            tracing::info!(
                network = %network,
                rpc = rpc_url,
                signers = ?signer_addresses,
                max_compute_unit_limit,
                max_compute_unit_price,
                "Initialized Solana provider"
            );
        }
        let rpc_client = RpcClient::new(rpc_url);
        Ok(Self {
            keypair: Arc::new(keypair),
            chain,
            rpc_client: Arc::new(rpc_client),
            max_compute_unit_limit,
            max_compute_unit_price,
        })
    }

    /// Get a reference to the RPC client (used for ERC-8004 read queries)
    pub fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    /// Get a reference to the keypair (used for ERC-8004 transaction signing)
    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    pub fn verify_compute_limit_instruction(
        &self,
        transaction: &VersionedTransaction,
        instruction_index: usize,
    ) -> Result<u32, FacilitatorLocalError> {
        let instructions = transaction.message.instructions();
        let instruction =
            instructions
                .get(instruction_index)
                .ok_or(FacilitatorLocalError::DecodingError(
                    "invalid_exact_svm_payload_transaction_instructions_length".to_string(),
                ))?;
        let account = instruction.program_id(transaction.message.static_account_keys());
        let compute_budget = solana_sdk::compute_budget::ID;
        let data = instruction.data.as_slice();

        // Verify program ID, discriminator, and data length (1 byte discriminator + 4 bytes u32)
        if compute_budget.ne(account) || data.first().cloned().unwrap_or(0) != 2 || data.len() != 5
        {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_compute_limit_instruction".to_string(),
            ));
        }

        // Parse compute unit limit (u32 in little-endian)
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&data[1..5]);
        let compute_units = u32::from_le_bytes(buf);

        Ok(compute_units)
    }

    pub fn verify_compute_price_instruction(
        &self,
        transaction: &VersionedTransaction,
        instruction_index: usize,
    ) -> Result<(), FacilitatorLocalError> {
        let instructions = transaction.message.instructions();
        let instruction =
            instructions
                .get(instruction_index)
                .ok_or(FacilitatorLocalError::DecodingError(
                    "invalid_exact_svm_payload_transaction_instructions_compute_price_instruction"
                        .to_string(),
                ))?;
        let account = instruction.program_id(transaction.message.static_account_keys());
        let compute_budget = solana_sdk::compute_budget::ID;
        let data = instruction.data.as_slice();
        if compute_budget.ne(account) || data.first().cloned().unwrap_or(0) != 3 || data.len() != 9
        {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_instructions_compute_price_instruction"
                    .to_string(),
            ));
        }
        // It is ComputeBudgetInstruction definitely by now!
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&data[1..]);
        let microlamports = u64::from_le_bytes(buf);
        if microlamports > self.max_compute_unit_price {
            return Err(FacilitatorLocalError::DecodingError(
                "compute unit price exceeds facilitator maximum".to_string(),
            ));
        }
        Ok(())
    }

    pub fn verify_create_ata_instruction(
        &self,
        transaction: &VersionedTransaction,
        index: usize,
        requirements: &PaymentRequirements,
    ) -> Result<(), FacilitatorLocalError> {
        let tx = TransactionInt::new(transaction.clone());
        let instruction = tx.instruction(index)?;
        instruction.assert_not_empty()?;

        // Verify program ID is the Associated Token Account Program
        let program_id = instruction.program_id();
        if program_id != ATA_PROGRAM_PUBKEY {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_create_ata_instruction".to_string(),
            ));
        }

        // Verify instruction discriminator
        // The ATA program's Create instruction has discriminator 0 (Create) or 1 (CreateIdempotent)
        let data = instruction.data_slice();
        if data.is_empty() || (data[0] != 0 && data[0] != 1) {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_create_ata_instruction".to_string(),
            ));
        }

        // Verify account count (must have at least 6 accounts)
        if instruction.instruction.accounts.len() < 6 {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_create_ata_instruction".to_string(),
            ));
        }

        // Payer = 0
        instruction.account(0)?;
        // ATA = 1
        instruction.account(1)?;
        // Owner = 2
        let owner = instruction.account(2)?;
        // Mint = 3
        let mint = instruction.account(3)?;
        // SystemProgram = 4
        instruction.account(4)?;
        // TokenProgram = 5
        instruction.account(5)?;

        // verify that the ATA is created for the expected payee
        let pay_to: SolanaAddress = requirements.pay_to.clone().try_into()?;
        if owner != pay_to.into() {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_create_ata_instruction_incorrect_payee"
                    .to_string(),
            ));
        }
        let asset: SolanaAddress = requirements.asset.clone().try_into()?;
        if mint != asset.into() {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_create_ata_instruction_incorrect_asset"
                    .to_string(),
            ));
        }

        Ok(())
    }

    // this expects the destination ATA to already exist
    pub async fn verify_transfer_instruction(
        &self,
        transaction: &VersionedTransaction,
        instruction_index: usize,
        requirements: &PaymentRequirements,
        has_dest_ata: bool,
    ) -> Result<TransferCheckedInstruction, FacilitatorLocalError> {
        let tx = TransactionInt::new(transaction.clone());
        let instruction = tx.instruction(instruction_index)?;
        instruction.assert_not_empty()?;
        let program_id = instruction.program_id();
        let transfer_checked_instruction = if spl_token::ID.eq(&program_id) {
            let token_instruction =
                spl_token::instruction::TokenInstruction::unpack(instruction.data_slice())
                    .map_err(|_| {
                        FacilitatorLocalError::DecodingError(
                            "invalid_exact_svm_payload_transaction_instructions".to_string(),
                        )
                    })?;
            let (amount, decimals) = match token_instruction {
                spl_token::instruction::TokenInstruction::TransferChecked { amount, decimals } => {
                    (amount, decimals)
                }
                _ => {
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_transaction_instructions".to_string(),
                    ));
                }
            };
            // Source = 0
            let source = instruction.account(0)?;
            // Mint = 1
            let mint = instruction.account(1)?;
            // Destination = 2
            let destination = instruction.account(2)?;
            // Authority = 3
            let authority = instruction.account(3)?;
            TransferCheckedInstruction {
                amount,
                decimals,
                source,
                mint,
                destination,
                authority,
                token_program: spl_token::ID,
                data: instruction.data(),
            }
        } else if spl_token_2022::ID.eq(&program_id) {
            let token_instruction =
                spl_token_2022::instruction::TokenInstruction::unpack(instruction.data_slice())
                    .map_err(|_| {
                        FacilitatorLocalError::DecodingError(
                            "invalid_exact_svm_payload_transaction_instructions".to_string(),
                        )
                    })?;
            let (amount, decimals) = match token_instruction {
                spl_token_2022::instruction::TokenInstruction::TransferChecked {
                    amount,
                    decimals,
                } => (amount, decimals),
                _ => {
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_transaction_instructions".to_string(),
                    ));
                }
            };
            // Source = 0
            let source = instruction.account(0)?;
            // Mint = 1
            let mint = instruction.account(1)?;
            // Destination = 2
            let destination = instruction.account(2)?;
            // Authority = 3
            let authority = instruction.account(3)?;
            TransferCheckedInstruction {
                amount,
                decimals,
                source,
                mint,
                destination,
                authority,
                token_program: spl_token_2022::ID,
                data: instruction.data(),
            }
        } else {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_not_a_transfer_instruction".to_string(),
            ));
        };

        // Verify that the fee payer is not transferring funds (not the authority)
        let fee_payer_pubkey = self.keypair.pubkey();
        if transfer_checked_instruction.authority == fee_payer_pubkey {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_fee_payer_transferring_funds".to_string(),
            ));
        }

        let asset_address: SolanaAddress = requirements.asset.clone().try_into()?;
        let pay_to_address: SolanaAddress = requirements.pay_to.clone().try_into()?;
        let token_program = transfer_checked_instruction.token_program;

        // SECURITY: Verify that the mint in the transaction matches the expected asset
        // This prevents attacks where someone sends a fake token with the same amount/destination
        if transfer_checked_instruction.mint != asset_address.pubkey {
            tracing::warn!(
                expected_mint = %asset_address.pubkey,
                actual_mint = %transfer_checked_instruction.mint,
                "Asset mismatch: transaction mint does not match expected asset"
            );
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_asset_mismatch".to_string(),
            ));
        }
        // findAssociatedTokenPda
        let (ata, _) = Pubkey::find_program_address(
            &[
                pay_to_address.pubkey.as_ref(),
                token_program.as_ref(),
                asset_address.pubkey.as_ref(),
            ],
            &ATA_PROGRAM_PUBKEY,
        );
        if transfer_checked_instruction.destination != ata {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_transfer_to_incorrect_ata".to_string(),
            ));
        }
        let accounts = self
            .rpc_client
            .get_multiple_accounts(&[transfer_checked_instruction.source, ata])
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;
        let is_sender_missing = accounts.first().cloned().is_none_or(|a| a.is_none());
        if is_sender_missing {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_sender_ata_not_found".to_string(),
            ));
        }
        let is_receiver_missing = accounts.get(1).cloned().is_none_or(|a| a.is_none());
        if is_receiver_missing && !has_dest_ata {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_receiver_ata_not_found".to_string(),
            ));
        }
        let instruction_amount: TokenAmount = transfer_checked_instruction.amount.into();
        let requirements_amount: TokenAmount = requirements.max_amount_required;
        if instruction_amount != requirements_amount {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_amount_mismatch".to_string(),
            ));
        }
        Ok(transfer_checked_instruction)
    }

    /// Find and verify the transfer instruction by scanning all instructions.
    /// Returns the index and parsed transfer instruction.
    /// This is flexible about instruction positions - Phantom may add extra instructions.
    async fn find_transfer_instruction(
        &self,
        transaction: &VersionedTransaction,
        requirements: &PaymentRequirements,
    ) -> Result<(usize, TransferCheckedInstruction), FacilitatorLocalError> {
        let instructions = transaction.message.instructions();
        let static_keys = transaction.message.static_account_keys();

        // Scan for transfer instruction by looking for spl_token or spl_token_2022 program
        for (idx, instruction) in instructions.iter().enumerate() {
            let program_id = instruction.program_id(static_keys);

            // Check if this is a token program instruction
            if *program_id != spl_token::ID && *program_id != spl_token_2022::ID {
                continue;
            }

            // Try to parse as TransferChecked
            let data = instruction.data.as_slice();
            if data.is_empty() {
                continue;
            }

            // TransferChecked discriminator is 12 for both spl_token and spl_token_2022
            if data[0] != 12 {
                continue;
            }

            // Check if there's a CreateATA instruction before this one
            let has_dest_ata = self.has_create_ata_before(transaction, idx, requirements);

            // Try to verify this as the transfer instruction
            match self
                .verify_transfer_instruction(transaction, idx, requirements, has_dest_ata)
                .await
            {
                Ok(transfer_instruction) => {
                    tracing::debug!(instruction_index = idx, "Found valid transfer instruction");
                    return Ok((idx, transfer_instruction));
                }
                Err(e) => {
                    tracing::debug!(
                        instruction_index = idx,
                        error = %e,
                        "Instruction at index is not the expected transfer"
                    );
                    continue;
                }
            }
        }

        Err(FacilitatorLocalError::DecodingError(
            "no_valid_transfer_instruction_found".to_string(),
        ))
    }

    /// Path 2: Find a TransferChecked instruction in CPI inner instructions from simulation.
    ///
    /// This enables smart wallet support (Squads, Crossmint, SWIG, etc.) where the token
    /// transfer is executed via Cross-Program Invocation rather than as a top-level instruction.
    ///
    /// Security hardening (audit fix F1):
    /// - Checks stack_height == 2 so a nested/shadow transfer at unexpected CPI depth is rejected.
    /// - Verifies the post-simulation ATA balance delta equals the required amount.
    /// - Warns (does not silently accept) when v0 ALT-resolved accounts cannot be fully verified.
    ///
    /// # Parameters
    ///
    /// * `inner_instructions` - Inner CPI instructions from simulation response.
    /// * `transaction` - The original versioned transaction.
    /// * `requirements` - Payment requirements (asset, pay_to, max_amount_required).
    /// * `pre_ata_balance` - On-chain ATA balance BEFORE simulation (fetched by caller).
    ///   `None` means the ATA does not yet exist (pre-balance = 0).
    /// * `post_ata_balance` - Post-simulation ATA balance from sim `accounts` response.
    ///   `None` means simulation did not return account data (see ALT warning below).
    fn find_transfer_in_inner_instructions(
        &self,
        inner_instructions: &[UiInnerInstructions],
        transaction: &VersionedTransaction,
        requirements: &PaymentRequirements,
        pre_ata_balance: Option<u64>,
        post_ata_balance: Option<u64>,
    ) -> Result<TransferCheckedInstruction, FacilitatorLocalError> {
        let static_keys = transaction.message.static_account_keys();
        let fee_payer_pubkey = self.keypair.pubkey();
        let asset_address: SolanaAddress = requirements.asset.clone().try_into()?;
        let pay_to_address: SolanaAddress = requirements.pay_to.clone().try_into()?;

        // Derive expected destination ATA for both token programs.
        let expected_ata_spl = Pubkey::find_program_address(
            &[
                pay_to_address.pubkey.as_ref(),
                spl_token::ID.as_ref(),
                asset_address.pubkey.as_ref(),
            ],
            &ATA_PROGRAM_PUBKEY,
        )
        .0;
        let expected_ata_2022 = Pubkey::find_program_address(
            &[
                pay_to_address.pubkey.as_ref(),
                spl_token_2022::ID.as_ref(),
                asset_address.pubkey.as_ref(),
            ],
            &ATA_PROGRAM_PUBKEY,
        )
        .0;

        // F1 ALT WARNING: for v0 transactions, the message may contain Address Lookup Tables
        // whose resolved accounts are NOT present in `static_account_keys`. The simulation
        // response does include resolved keys in the execution context, but the
        // RpcSimulatedTransactionResult type returned by solana_rpc_client does not expose
        // a resolved-key list separate from the transaction's static_account_keys slice.
        // As a result, accounts referenced via ALT indices in compiled.accounts may fail
        // to resolve (will be skipped), or may coincidentally resolve to a wrong key.
        //
        // TODO(audit:F1): implement full ALT resolution by calling getAddressLookupTable for
        // each ALT in the v0 message header, building the full resolved key table, and
        // re-running the account index lookups against that table. Until then, a v0 transaction
        // that hides the transfer destination in an ALT will fail instruction validation
        // (destination won't match expected_ata) and be correctly rejected. The risk is
        // a false-negative for legitimate v0 smart wallets using ALTs -- they would be rejected.
        // This is the SAFER failure mode for a payment validator.
        //
        // Detection: address_table_lookups() returns Some only for v0 messages.
        let has_alt_lookups = transaction
            .message
            .address_table_lookups()
            .map(|alts| !alts.is_empty())
            .unwrap_or(false);
        if has_alt_lookups {
            tracing::warn!(
                "Path 2 CPI scan: transaction uses v0 message with Address Lookup Tables. \
                 Account index resolution is limited to static_account_keys only. \
                 ALT-resolved accounts will fail index lookup and be skipped (safer failure mode). \
                 See audit finding F1 for full remediation plan."
            );
        }

        // F1 stack_height: We require the matching TransferChecked to be at CPI depth 2.
        // Depth 1 = top-level instruction (handled by Path 1, not here).
        // Depth 2 = one CPI level deep, expected for smart wallet dispatch (Squads/Crossmint/SWIG).
        // A transfer appearing at depth 3+ could indicate an attacker-nested transfer; we reject it.
        // Some RPC nodes do not populate stack_height (returns None). In that case we accept the
        // instruction but log a warning so operators can audit this case.
        const EXPECTED_CPI_STACK_HEIGHT: u32 = 2;

        let mut found: Option<TransferCheckedInstruction> = None;

        for group in inner_instructions {
            for ui_ix in &group.instructions {
                let compiled = match ui_ix {
                    UiInstruction::Compiled(c) => c,
                    // Parsed instructions are returned when the RPC uses jsonParsed encoding;
                    // our simulation uses the default binary encoding so this branch is
                    // unexpected, but skip gracefully if it occurs.
                    UiInstruction::Parsed(_) => continue,
                };

                // Resolve program ID from the transaction's account keys.
                let program_id = match static_keys.get(compiled.program_id_index as usize) {
                    Some(pk) => *pk,
                    None => continue,
                };

                // Only look at spl_token and spl_token_2022 programs.
                if program_id != spl_token::ID && program_id != spl_token_2022::ID {
                    continue;
                }

                // Decode bs58 instruction data.
                let data = match bs58::decode(&compiled.data).into_vec() {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                // TransferChecked discriminator = 12, needs at least 10 bytes (1 + 8 + 1).
                if data.is_empty() || data[0] != 12 || data.len() < 10 {
                    continue;
                }

                // Parse amount (u64 LE) and decimals (u8).
                let mut amount_buf = [0u8; 8];
                amount_buf.copy_from_slice(&data[1..9]);
                let amount = u64::from_le_bytes(amount_buf);
                let decimals = data[9];

                // Resolve account keys: source(0), mint(1), destination(2), authority(3).
                if compiled.accounts.len() < 4 {
                    continue;
                }
                let resolve = |idx: usize| -> Option<Pubkey> {
                    static_keys.get(compiled.accounts[idx] as usize).copied()
                };
                let (source, mint, destination, authority) =
                    match (resolve(0), resolve(1), resolve(2), resolve(3)) {
                        (Some(s), Some(m), Some(d), Some(a)) => (s, m, d, a),
                        // Unresolvable index -- likely an ALT reference. Skip with a debug log;
                        // the v0 warning above already notified the operator.
                        _ => {
                            tracing::debug!(
                                "Path 2: could not resolve account index in CPI instruction -- \
                                 ALT reference or out-of-range index, skipping"
                            );
                            continue;
                        }
                    };

                // Validate: mint must match expected asset.
                if mint != asset_address.pubkey {
                    continue;
                }

                // Validate: destination must be the correct ATA.
                if destination != expected_ata_spl && destination != expected_ata_2022 {
                    continue;
                }

                // Validate: amount must match requirements.
                let instruction_amount: TokenAmount = amount.into();
                let requirements_amount: TokenAmount = requirements.max_amount_required;
                if instruction_amount != requirements_amount {
                    continue;
                }

                // Security: authority must not be the fee payer.
                if authority == fee_payer_pubkey {
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_inner_transfer_fee_payer_is_authority"
                            .to_string(),
                    ));
                }

                // F1 STACK HEIGHT CHECK: reject transfers nested deeper than expected CPI depth.
                match compiled.stack_height {
                    Some(depth) if depth != EXPECTED_CPI_STACK_HEIGHT => {
                        tracing::warn!(
                            found_depth = depth,
                            expected_depth = EXPECTED_CPI_STACK_HEIGHT,
                            "Path 2: TransferChecked found at unexpected CPI stack depth -- rejecting"
                        );
                        return Err(FacilitatorLocalError::DecodingError(
                            "invalid_exact_svm_payload_inner_transfer_unexpected_stack_depth"
                                .to_string(),
                        ));
                    }
                    None => {
                        tracing::warn!(
                            "Path 2: RPC did not return stack_height for CPI TransferChecked -- \
                             accepting but operator should verify RPC node version supports stack_height"
                        );
                    }
                    Some(_) => {
                        // Correct depth; proceed.
                    }
                }

                // Ensure exactly ONE matching TransferChecked.
                if found.is_some() {
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_multiple_inner_transfers_found".to_string(),
                    ));
                }

                tracing::info!(
                    amount = amount,
                    mint = %mint,
                    destination = %destination,
                    authority = %authority,
                    token_program = %program_id,
                    "Found CPI TransferChecked in inner instructions (smart wallet path)"
                );

                found = Some(TransferCheckedInstruction {
                    amount,
                    decimals,
                    source,
                    mint,
                    destination,
                    authority,
                    token_program: program_id,
                    data,
                });
            }
        }

        let transfer = found.ok_or_else(|| {
            FacilitatorLocalError::DecodingError(
                "no_valid_transfer_in_inner_instructions".to_string(),
            )
        })?;

        // F1 POST-BALANCE DELTA CHECK: verify simulation confirms the ATA balance increased
        // by exactly the required amount.  This catches cases where an attacker constructs a
        // transaction that contains a valid-looking TransferChecked instruction but also
        // contains a balancing debit (CloseAccount, Transfer-out) that leaves the merchant
        // ATA balance unchanged.
        //
        // Use to_string().parse() to convert U256 -> u64, matching the pattern used in the
        // settlement-account verify path (requirements.max_amount_required is a U256 newtype).
        let required_amount: u64 = requirements
            .max_amount_required
            .0
            .to_string()
            .parse()
            .unwrap_or(0);
        match (pre_ata_balance, post_ata_balance) {
            (Some(pre), Some(post)) => {
                let delta = post.saturating_sub(pre);
                if delta != required_amount {
                    tracing::warn!(
                        pre_balance = pre,
                        post_balance = post,
                        delta = delta,
                        required = required_amount,
                        "Path 2: post-simulation ATA balance delta does not match required amount"
                    );
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_inner_transfer_balance_delta_mismatch"
                            .to_string(),
                    ));
                }
                tracing::debug!(
                    pre_balance = pre,
                    post_balance = post,
                    delta = delta,
                    "Path 2: ATA balance delta check passed"
                );
            }
            (None, Some(post)) => {
                // ATA did not exist before tx; post-balance must equal required amount exactly.
                if post != required_amount {
                    tracing::warn!(
                        post_balance = post,
                        required = required_amount,
                        "Path 2: new ATA post-simulation balance does not equal required amount"
                    );
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_inner_transfer_balance_delta_mismatch"
                            .to_string(),
                    ));
                }
            }
            (_, None) => {
                // Simulation did not return account data for the ATA (e.g., ATA is newly created
                // and not in the simulation accounts response, or the RPC node does not support it).
                // This can happen legitimately when the ATA is created in the same tx.
                // Log and allow to avoid breaking the production Crossmint flow.
                // TODO(audit:F1): investigate whether this case occurs on mainnet and tighten
                // if simulation reliably returns account data for newly created ATAs.
                tracing::warn!(
                    "Path 2: simulation did not return post-balance for pay_to ATA -- \
                     skipping balance delta check. Review if this occurs in production."
                );
            }
        }

        Ok(transfer)
    }

    /// Check if there's a CreateATA instruction before the given index
    /// that creates the ATA for the payee
    fn has_create_ata_before(
        &self,
        transaction: &VersionedTransaction,
        transfer_idx: usize,
        requirements: &PaymentRequirements,
    ) -> bool {
        let instructions = transaction.message.instructions();

        for idx in 0..transfer_idx {
            if let Some(instruction) = instructions.get(idx) {
                let program_id = instruction.program_id(transaction.message.static_account_keys());
                if *program_id == ATA_PROGRAM_PUBKEY {
                    // Verify it's creating the right ATA
                    if self
                        .verify_create_ata_instruction(transaction, idx, requirements)
                        .is_ok()
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Verify compute budget instructions exist and are within limits.
    /// Flexible about exact positions - just needs to find them.
    fn verify_compute_budget_instructions(
        &self,
        transaction: &VersionedTransaction,
    ) -> Result<(), FacilitatorLocalError> {
        let instructions = transaction.message.instructions();
        let static_keys = transaction.message.static_account_keys();
        let compute_budget_id = solana_sdk::compute_budget::ID;

        let mut found_limit = false;
        let mut found_price = false;

        for (idx, instruction) in instructions.iter().enumerate() {
            let program_id = instruction.program_id(static_keys);
            if *program_id != compute_budget_id {
                continue;
            }

            let data = instruction.data.as_slice();
            if data.is_empty() {
                continue;
            }

            match data[0] {
                2 if data.len() == 5 => {
                    // SetComputeUnitLimit - reject duplicates (Solana applies last-wins)
                    if found_limit {
                        return Err(FacilitatorLocalError::DecodingError(
                            "duplicate_compute_unit_limit_instruction".to_string(),
                        ));
                    }
                    let mut buf = [0u8; 4];
                    buf.copy_from_slice(&data[1..5]);
                    let compute_units = u32::from_le_bytes(buf);
                    if compute_units > self.max_compute_unit_limit {
                        return Err(FacilitatorLocalError::DecodingError(
                            "compute unit limit exceeds facilitator maximum".to_string(),
                        ));
                    }
                    tracing::debug!(
                        instruction_index = idx,
                        compute_units = compute_units,
                        "Found compute unit limit instruction"
                    );
                    found_limit = true;
                }
                3 if data.len() == 9 => {
                    // SetComputeUnitPrice - reject duplicates (Solana applies last-wins)
                    if found_price {
                        return Err(FacilitatorLocalError::DecodingError(
                            "duplicate_compute_unit_price_instruction".to_string(),
                        ));
                    }
                    let mut buf = [0u8; 8];
                    buf.copy_from_slice(&data[1..]);
                    let microlamports = u64::from_le_bytes(buf);
                    if microlamports > self.max_compute_unit_price {
                        return Err(FacilitatorLocalError::DecodingError(
                            "compute unit price exceeds facilitator maximum".to_string(),
                        ));
                    }
                    tracing::debug!(
                        instruction_index = idx,
                        microlamports = microlamports,
                        "Found compute unit price instruction"
                    );
                    found_price = true;
                }
                _ => {}
            }
        }

        if !found_limit {
            return Err(FacilitatorLocalError::DecodingError(
                "missing_compute_unit_limit_instruction".to_string(),
            ));
        }
        if !found_price {
            return Err(FacilitatorLocalError::DecodingError(
                "missing_compute_unit_price_instruction".to_string(),
            ));
        }

        Ok(())
    }

    /// Refuse a Token-2022 mint whose transfer fee would pay the payee short.
    ///
    /// `TransferChecked` on a mint with a `TransferFeeConfig` debits the payer
    /// `amount` but credits the payee `amount - fee` (the fee stays withheld in
    /// the destination account). Every amount check in verify compares the
    /// INSTRUCTION amount, so a non-zero fee would turn an exact payment into a
    /// short one that still verifies. PYUSD and AUSD both carry the extension at
    /// 0 (measured 2026-09-13) and both issuers keep the authority to change it,
    /// so the mint is read on every verify, and settle re-runs verify.
    ///
    /// The mint and the Clock sysvar come from one `getMultipleAccounts`, so the
    /// epoch belongs to the same slot as the config it is compared against. An
    /// RPC failure refuses the payment: no verdict is not a zero fee.
    async fn verify_mint_charges_no_transfer_fee(
        &self,
        mint: &Pubkey,
        amount: u64,
    ) -> Result<(), FacilitatorLocalError> {
        let accounts = self
            .rpc_client
            .get_multiple_accounts(&[*mint, solana_sdk::sysvar::clock::ID])
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;
        let mint_account = accounts.first().cloned().flatten().ok_or_else(|| {
            FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_mint_not_found".to_string(),
            )
        })?;
        let clock = accounts
            .get(1)
            .cloned()
            .flatten()
            .and_then(|account| bincode::deserialize::<Clock>(&account.data).ok())
            .ok_or_else(|| {
                FacilitatorLocalError::ContractCall("clock sysvar unavailable".to_string())
            })?;
        let fee =
            transfer_fee_upper_bound(&mint_account.owner, &mint_account.data, clock.epoch, amount)?;
        if fee != 0 {
            tracing::warn!(
                mint = %mint,
                amount,
                fee,
                epoch = clock.epoch,
                "Token-2022 mint charges a transfer fee: the payee would receive less than the signed amount"
            );
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_transfer_fee_not_zero".to_string(),
            ));
        }
        Ok(())
    }

    async fn verify_transfer(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerifyTransferResult, FacilitatorLocalError> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;

        // Assert valid payment START
        let payment_payload = match &payload.payload {
            #[cfg(feature = "hedera")]
            ExactPaymentPayload::Hedera(_) => return Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            ExactPaymentPayload::Evm(..) => {
                return Err(FacilitatorLocalError::UnsupportedNetwork(None));
            }
            ExactPaymentPayload::Near(..) => {
                return Err(FacilitatorLocalError::UnsupportedNetwork(None));
            }
            ExactPaymentPayload::Stellar(..) => {
                return Err(FacilitatorLocalError::UnsupportedNetwork(None));
            }
            #[cfg(feature = "algorand")]
            ExactPaymentPayload::Algorand(..) => {
                return Err(FacilitatorLocalError::UnsupportedNetwork(None));
            }
            #[cfg(feature = "sui")]
            ExactPaymentPayload::Sui(..) => {
                return Err(FacilitatorLocalError::UnsupportedNetwork(None));
            }
            ExactPaymentPayload::SolanaSettlementAccount(..) => {
                // Settlement account payloads are handled by verify_settlement_account,
                // not verify_transfer. This branch shouldn't be reached.
                return Err(FacilitatorLocalError::DecodingError(
                    "settlement account payload should not reach verify_transfer".to_string(),
                ));
            }
            #[cfg(feature = "xrpl")]
            ExactPaymentPayload::Xrpl(..) => {
                return Err(FacilitatorLocalError::UnsupportedNetwork(None));
            }
            ExactPaymentPayload::Solana(payload) => payload,
        };
        if payload.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                payload.network,
            ));
        }
        if requirements.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                requirements.network,
            ));
        }
        if payload.scheme != requirements.scheme {
            return Err(FacilitatorLocalError::SchemeMismatch(
                None,
                requirements.scheme,
                payload.scheme,
            ));
        }

        // Decode the transaction exactly as the user signed it
        // This preserves all instructions including any added by Phantom
        let transaction_b64_string = payment_payload.transaction.clone();
        let bytes = Base64Bytes::from(transaction_b64_string.as_bytes())
            .decode()
            .map_err(|e| FacilitatorLocalError::DecodingError(format!("{e}")))?;
        let transaction = bincode::deserialize::<VersionedTransaction>(bytes.as_slice())
            .map_err(|e| FacilitatorLocalError::DecodingError(format!("{e}")))?;

        tracing::debug!(
            num_instructions = transaction.message.instructions().len(),
            num_signatures = transaction.signatures.len(),
            "Decoded user-signed transaction"
        );

        // Flexible verification: find instructions by program ID, not fixed positions
        // This allows Phantom to add extra instructions while we still validate the critical ones

        // 1. Verify compute budget instructions exist and are within limits
        self.verify_compute_budget_instructions(&transaction)?;

        // 2. Fee payer safety check
        // Verify that the fee payer is not included in any instruction's accounts
        // This single check covers all cases: authority, source, or any other role
        let fee_payer_pubkey = self.keypair.pubkey();
        for instruction in transaction.message.instructions().iter() {
            for account_idx in instruction.accounts.iter() {
                let account = transaction
                    .message
                    .static_account_keys()
                    .get(*account_idx as usize)
                    .ok_or(FacilitatorLocalError::DecodingError(
                        "invalid_account_index".to_string(),
                    ))?;

                if *account == fee_payer_pubkey {
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_transaction_fee_payer_included_in_instruction_accounts".to_string(),
                    ));
                }
            }
        }

        // 3. Try Path 1: Find top-level TransferChecked instruction (standard wallets)
        let top_level_result = self
            .find_transfer_instruction(&transaction, requirements)
            .await;

        // 4. Simulate the transaction (with our signature added).
        //
        // We request both expected ATAs (spl and spl-2022) in the `accounts` field so that
        // the simulation returns post-execution account state.  This is used by Path 2 to
        // verify the ATA balance delta (audit fix F1).
        //
        // The `accounts` list is positional: index 0 = expected_ata_spl, index 1 = expected_ata_2022.
        let asset_address: SolanaAddress = requirements.asset.clone().try_into()?;
        let pay_to_address: SolanaAddress = requirements.pay_to.clone().try_into()?;
        let expected_ata_spl = Pubkey::find_program_address(
            &[
                pay_to_address.pubkey.as_ref(),
                spl_token::ID.as_ref(),
                asset_address.pubkey.as_ref(),
            ],
            &ATA_PROGRAM_PUBKEY,
        )
        .0;
        let expected_ata_2022 = Pubkey::find_program_address(
            &[
                pay_to_address.pubkey.as_ref(),
                spl_token_2022::ID.as_ref(),
                asset_address.pubkey.as_ref(),
            ],
            &ATA_PROGRAM_PUBKEY,
        )
        .0;

        // Fetch pre-simulation ATA balances for Path 2 delta check.
        // We request both ATAs. A missing account (Err from get_token_account_balance)
        // means the ATA does not yet exist, which we treat as balance 0.
        let pre_balance_spl = self
            .rpc_client
            .get_token_account_balance(&expected_ata_spl)
            .await
            .ok()
            .and_then(|b| b.amount.parse::<u64>().ok());
        let pre_balance_2022 = self
            .rpc_client
            .get_token_account_balance(&expected_ata_2022)
            .await
            .ok()
            .and_then(|b| b.amount.parse::<u64>().ok());

        // Enable inner_instructions to support smart wallet CPI detection (Path 2).
        // Request both expected ATA addresses so we get post-simulation balances.
        let tx = TransactionInt::new(transaction.clone()).sign(&self.keypair)?;
        let cfg = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: false,
            commitment: Some(CommitmentConfig::confirmed()),
            encoding: None,
            accounts: Some(RpcSimulateTransactionAccountsConfig {
                encoding: None, // defaults to base58 / LegacyBinary; decoded via .decode()
                addresses: vec![expected_ata_spl.to_string(), expected_ata_2022.to_string()],
            }),
            inner_instructions: true,
            min_context_slot: None,
        };
        let sim = self
            .rpc_client
            .simulate_transaction_with_config(&tx.inner, cfg)
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;
        if sim.value.err.is_some() {
            tracing::warn!(
                error = ?sim.value.err,
                logs = ?sim.value.logs,
                "Transaction simulation failed"
            );
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_simulation_failed".to_string(),
            ));
        }

        // Parse post-simulation ATA balances from the accounts response.
        // Index 0 = expected_ata_spl, index 1 = expected_ata_2022.
        let sim_accounts = sim.value.accounts.as_deref().unwrap_or(&[]);
        let post_balance_from_account = |account_opt: Option<
            &Option<solana_account_decoder_client_types::UiAccount>,
        >|
         -> Option<u64> {
            let account = account_opt?.as_ref()?;
            // UiAccount.data is a UiAccountData; .decode() returns Option<Vec<u8>>.
            let raw = account.data.decode()?;
            parse_token_account_balance(&raw)
        };
        let post_balance_spl = post_balance_from_account(sim_accounts.first());
        let post_balance_2022 = post_balance_from_account(sim_accounts.get(1));

        // Determine which ATA we are working with, combining pre and post balances.
        // Heuristic: use the ATA that has a non-zero post-balance (or a delta from pre).
        let (pre_ata_balance, post_ata_balance) = {
            let delta_spl = post_balance_spl
                .unwrap_or(0)
                .saturating_sub(pre_balance_spl.unwrap_or(0));
            let delta_2022 = post_balance_2022
                .unwrap_or(0)
                .saturating_sub(pre_balance_2022.unwrap_or(0));
            if delta_2022 > delta_spl {
                (pre_balance_2022, post_balance_2022)
            } else {
                (pre_balance_spl, post_balance_spl)
            }
        };

        // 5. Determine transfer instruction via Path 1 or Path 2
        let transfer_in_cpi = top_level_result.is_err();
        let transfer_instruction = match top_level_result {
            Ok((_idx, ti)) => {
                tracing::debug!("Path 1: Found top-level TransferChecked (standard wallet)");
                ti
            }
            Err(path1_err) => {
                // Path 2: Smart wallet - find TransferChecked in CPI inner instructions
                let inner_ixs = sim.value.inner_instructions.as_deref().unwrap_or(&[]);
                if inner_ixs.is_empty() {
                    tracing::warn!(
                        path1_error = %path1_err,
                        "No top-level transfer and no inner instructions from simulation"
                    );
                    return Err(path1_err);
                }

                match self.find_transfer_in_inner_instructions(
                    inner_ixs,
                    &transaction,
                    requirements,
                    pre_ata_balance,
                    post_ata_balance,
                ) {
                    Ok(ti) => {
                        tracing::info!(
                            authority = %ti.authority,
                            "Path 2: Found CPI TransferChecked in inner instructions (smart wallet)"
                        );
                        ti
                    }
                    Err(path2_err) => {
                        tracing::warn!(
                            path1_error = %path1_err,
                            path2_error = %path2_err,
                            "Neither top-level nor inner instruction transfer found"
                        );
                        return Err(FacilitatorLocalError::DecodingError(
                            "no_valid_transfer_found_in_top_level_or_inner_instructions"
                                .to_string(),
                        ));
                    }
                }
            }
        };

        // 6. Token-2022 transfer fee: both paths above matched the instruction
        // amount, which is what the payer sends, not what the payee receives.
        if transfer_instruction.token_program == spl_token_2022::ID {
            self.verify_mint_charges_no_transfer_fee(
                &transfer_instruction.mint,
                transfer_instruction.amount,
            )
            .await?;
        }

        let payer: SolanaAddress = transfer_instruction.authority.into();
        Ok(VerifyTransferResult {
            payer,
            transaction,
            transfer_in_cpi,
        })
    }

    /// Reads a confirmed transaction back from the chain and judges what it
    /// paid payTo (`landed_payment`). A read that gets no transaction is asked
    /// again, `LANDED_READ_INTERVAL` apart; after `LANDED_READ_ATTEMPTS` the
    /// answer is no verdict.
    async fn read_landed_payment(
        &self,
        signature: &Signature,
        requirements: &PaymentRequirements,
    ) -> LandedPayment {
        use solana_client::rpc_config::RpcTransactionConfig;
        use solana_transaction_status_client_types::UiTransactionEncoding;

        let (pay_to, mint) = match (
            SolanaAddress::try_from(requirements.pay_to.clone()),
            SolanaAddress::try_from(requirements.asset.clone()),
        ) {
            (Ok(pay_to), Ok(mint)) => (pay_to.pubkey, mint.pubkey),
            _ => {
                return LandedPayment::Unknown(
                    "payTo or the asset is not a Solana address".to_string(),
                )
            }
        };
        let Ok(required) = requirements
            .max_amount_required
            .0
            .to_string()
            .parse::<u64>()
        else {
            return LandedPayment::Unknown("maxAmountRequired is not a u64".to_string());
        };
        let config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Base64),
            commitment: Some(CommitmentConfig::confirmed()),
            max_supported_transaction_version: Some(0),
        };
        let mut last_error = String::new();
        for attempt in 0..LANDED_READ_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(LANDED_READ_INTERVAL).await;
            }
            match self
                .rpc_client
                .get_transaction_with_config(signature, config)
                .await
            {
                Ok(landed) => {
                    return match landed.transaction.meta {
                        Some(meta) => landed_payment(&meta, &pay_to, &mint, required),
                        None => {
                            LandedPayment::Unknown("the transaction carries no status".to_string())
                        }
                    }
                }
                Err(e) => last_error = crate::redact::scrub_urls(&e.to_string()),
            }
        }
        LandedPayment::Unknown(format!(
            "not handed back in {LANDED_READ_ATTEMPTS} reads: {last_error}"
        ))
    }

    /// Every signature the transaction requires, except the fee payer's (settle
    /// adds it), is present and verifies against its message.
    ///
    /// verify simulates without checking signatures, so without this a
    /// transaction its payer never signed verifies and only fails at settle.
    fn verify_signed_by_its_signers(
        &self,
        transaction: &VersionedTransaction,
    ) -> Result<(), FacilitatorLocalError> {
        let message = transaction.message.serialize();
        let required = transaction.message.header().num_required_signatures as usize;
        let fee_payer = self.keypair.pubkey();
        for (position, signer) in transaction
            .message
            .static_account_keys()
            .iter()
            .take(required)
            .enumerate()
        {
            if *signer == fee_payer {
                continue;
            }
            let signed = transaction
                .signatures
                .get(position)
                .is_some_and(|signature| signature.verify(signer.as_ref(), &message));
            if !signed {
                tracing::warn!(
                    signer = %signer,
                    position,
                    "Solana payment not signed by a signer it requires"
                );
                return Err(FacilitatorLocalError::DecodingError(
                    "invalid_exact_svm_payload_transaction_signature_invalid".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn fee_payer(&self) -> MixedAddress {
        let pubkey = self.keypair.pubkey();
        MixedAddress::Solana(pubkey)
    }

    // ========================================================================
    // Settlement Account Support (Crossmint custodial wallets)
    // ========================================================================

    /// Verify a settlement account payment by checking the on-chain transaction.
    ///
    /// The custodial wallet already submitted the transaction. We fetch it from
    /// the RPC and verify it transferred sufficient USDC to the merchant's ATA.
    ///
    /// Security model (audit fix B1):
    ///
    /// 1. OWNER CHECK: only credits whose token balance record names the account that
    ///    will hold the payment count. Without a `settleSecretKey` that is
    ///    `requirements.pay_to`; with one, it is the settlement keypair the key carries,
    ///    and the settlement ATA must also hold at least the price now.
    ///
    /// 2. The `settleSecretKey` model (private key transmitted over the wire) is a
    ///    residual risk that persists after this fix. See `SOLANA_NONCE_STORE` comment
    ///    block above for the recommended PDA migration.
    async fn verify_settlement_account(
        &self,
        payload: &crate::types::SettlementAccountPayload,
        requirements: &PaymentRequirements,
    ) -> Result<SettlementAccountVerifyResult, FacilitatorLocalError> {
        use solana_client::rpc_config::RpcTransactionConfig;
        use solana_transaction_status_client_types::UiTransactionEncoding;

        let sig = Signature::from_str(&payload.transaction_signature).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("invalid transaction signature: {e}"))
        })?;

        tracing::info!(
            network = %self.network(),
            tx_signature = %sig,
            "Verifying settlement account on-chain transaction"
        );

        // Fetch the transaction with token balance info
        let config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::JsonParsed),
            commitment: Some(CommitmentConfig::confirmed()),
            max_supported_transaction_version: Some(0),
        };

        // Retry fetching the transaction (it may not be indexed yet)
        let mut tx_info = None;
        for attempt in 0..10 {
            match self
                .rpc_client
                .get_transaction_with_config(&sig, config)
                .await
            {
                Ok(info) => {
                    tx_info = Some(info);
                    break;
                }
                Err(e) => {
                    if attempt < 9 {
                        tracing::debug!(
                            attempt = attempt + 1,
                            error = %e,
                            "Transaction not yet available, retrying..."
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }

        let tx_info = tx_info.ok_or_else(|| {
            FacilitatorLocalError::ContractCall(
                "settlement account transaction not found on-chain after 20s".to_string(),
            )
        })?;

        // Check transaction succeeded
        if let Some(ref meta) = tx_info.transaction.meta {
            if meta.err.is_some() {
                return Err(FacilitatorLocalError::ContractCall(format!(
                    "settlement account transaction failed on-chain: {:?}",
                    meta.err
                )));
            }
        } else {
            return Err(FacilitatorLocalError::ContractCall(
                "settlement account transaction has no metadata".to_string(),
            ));
        }

        let meta = tx_info.transaction.meta.as_ref().unwrap();

        // Parse the required asset (USDC mint) from requirements
        let asset_pubkey: Pubkey = match &requirements.asset {
            MixedAddress::Solana(pk) => *pk,
            _ => {
                return Err(FacilitatorLocalError::InvalidAddress(
                    "expected Solana asset address".to_string(),
                ))
            }
        };

        // Parse required amount
        let required_amount: u64 = requirements
            .max_amount_required
            .0
            .to_string()
            .parse::<u64>()
            .map_err(|e| {
                FacilitatorLocalError::DecodingError(format!(
                    "cannot parse maxAmountRequired as u64: {e}"
                ))
            })?;

        // Check pre/post token balances for USDC transfer
        let pre_balances = meta
            .pre_token_balances
            .as_ref()
            .map(|b| b.as_slice())
            .unwrap_or(&[]);
        let post_balances = meta
            .post_token_balances
            .as_ref()
            .map(|b| b.as_slice())
            .unwrap_or(&[]);

        let asset_str = asset_pubkey.to_string();

        // SECURITY (audit 03): resolve pay_to so we can separately track credit that lands
        // DIRECTLY in the merchant's wallet. In the no-sweep (settleSecretKey == None) path,
        // funds reach pay_to ONLY if the referenced tx credited pay_to directly -- we require
        // that below. (When a settleSecretKey IS supplied, the sweep moves funds from the
        // settlement ATA to pay_to, so the credit that counts is the one to the settlement ATA.)
        let pay_to_str: Option<String> = match &requirements.pay_to {
            MixedAddress::Solana(pk) => Some(pk.to_string()),
            _ => None,
        };

        // The Crossmint settlement-account model deposits USDC into a settlement ATA (owned by
        // the settlement keypair), NOT directly into pay_to's ATA, and the sweep then moves it
        // from there to pay_to. So with a settleSecretKey the credit that counts is the one to
        // the account that key controls -- the account the sweep pays out of -- never a credit
        // to any other account in the cited transaction. The settlement keypair is decoded the
        // same way the sweep decodes it (`settlement_keypair`).
        //
        // RECOMMENDED NEXT STEP: Migrate to a PDA model where the settlement address is derived
        // from (payer, pay_to, nonce) and the facilitator controls the PDA.  This removes the
        // settleSecretKey entirely.  See SOLANA_NONCE_STORE comment block for details.
        let settlement_owner: Option<Pubkey> = match &payload.settle_secret_key {
            Some(secret_key_str) => Some(settlement_keypair(secret_key_str)?.pubkey()),
            None => None,
        };
        let settlement_owner_str: Option<String> = settlement_owner.map(|pk| pk.to_string());
        let mut total_credit: u64 = 0;
        // SECURITY (audit 03): credit that landed DIRECTLY in pay_to's ATA (owner == pay_to).
        let mut pay_to_credit: u64 = 0;
        // Credit that landed in an account owned by the settlement keypair.
        let mut settlement_credit: u64 = 0;
        let mut payer_pubkey: Option<Pubkey> = None;

        for post_bal in post_balances {
            // Filter by mint.
            if post_bal.mint != asset_str {
                continue;
            }

            let post_amount: u64 = post_bal.ui_token_amount.amount.parse().unwrap_or(0);

            // Find matching pre-balance.
            let pre_amount: u64 = pre_balances
                .iter()
                .find(|p| p.account_index == post_bal.account_index && p.mint == asset_str)
                .map(|p| p.ui_token_amount.amount.parse().unwrap_or(0))
                .unwrap_or(0);

            let diff = post_amount.saturating_sub(pre_amount);
            if diff > 0 {
                total_credit += diff;
                // Track credit that landed directly in pay_to's ATA.
                if let (OptionSerializer::Some(ref owner), Some(ref pay_to)) =
                    (&post_bal.owner, &pay_to_str)
                {
                    if owner == pay_to {
                        pay_to_credit += diff;
                    }
                }
                if let (OptionSerializer::Some(ref owner), Some(ref settlement)) =
                    (&post_bal.owner, &settlement_owner_str)
                {
                    if owner == settlement {
                        settlement_credit += diff;
                    }
                }
                tracing::debug!(
                    account_index = post_bal.account_index,
                    credit = diff,
                    owner = ?post_bal.owner,
                    "Found USDC credit in settlement transaction"
                );
            }

            // Track the source (debit) as the payer.
            if pre_amount > post_amount {
                if let OptionSerializer::Some(ref owner) = post_bal.owner {
                    if let Ok(pk) = Pubkey::from_str(owner) {
                        payer_pubkey = Some(pk);
                    }
                }
            }
        }

        // Also check debits to find the payer.
        if payer_pubkey.is_none() {
            for pre_bal in pre_balances {
                if pre_bal.mint != asset_str {
                    continue;
                }
                let pre_amount: u64 = pre_bal.ui_token_amount.amount.parse().unwrap_or(0);
                let post_amount: u64 = post_balances
                    .iter()
                    .find(|p| p.account_index == pre_bal.account_index && p.mint == asset_str)
                    .map(|p| p.ui_token_amount.amount.parse().unwrap_or(0))
                    .unwrap_or(0);

                if pre_amount > post_amount {
                    if let OptionSerializer::Some(ref owner) = pre_bal.owner {
                        if let Ok(pk) = Pubkey::from_str(owner) {
                            payer_pubkey = Some(pk);
                        }
                    }
                }
            }
        }

        if total_credit < required_amount {
            return Err(FacilitatorLocalError::DecodingError(format!(
                "settlement account transfer amount {} < required {}",
                total_credit, required_amount
            )));
        }

        // SECURITY (audit 03): when there is NO sweep (settleSecretKey == None), the facilitator
        // does not move any funds itself, so the merchant is paid ONLY if the referenced tx
        // credited pay_to's ATA directly. Without this, an attacker could cite ANY unrelated
        // confirmed USDC transfer (crediting strangers' ATAs) and the facilitator would forge a
        // success while pay_to received nothing. When a settleSecretKey IS supplied, the sweep
        // (to pay_to) provides the binding, and the settlement ATA is checked right below.
        if payload.settle_secret_key.is_none() && pay_to_credit < required_amount {
            return Err(FacilitatorLocalError::DecodingError(format!(
                "settlement account (no sweep): credit to pay_to {} < required {} \
                 (referenced transaction did not pay the merchant directly)",
                pay_to_credit, required_amount
            )));
        }

        // SECURITY: when a settleSecretKey IS supplied, the sweep pays pay_to out of the
        // settlement ATA, so the referenced tx must have deposited the price there, and the
        // ATA must still hold it now: the sweep moves the price or nothing, and verify
        // answers what settle would. Read at the commitment the tx itself was read at.
        if let Some(settlement_owner) = settlement_owner {
            if settlement_credit < required_amount {
                return Err(FacilitatorLocalError::DecodingError(format!(
                    "settlement account (sweep): credit to the settlement account {} < required {} \
                     (referenced transaction did not fund it)",
                    settlement_credit, required_amount
                )));
            }
            let (settlement_ata, _) = Pubkey::find_program_address(
                &[
                    settlement_owner.as_ref(),
                    spl_token::id().as_ref(),
                    asset_pubkey.as_ref(),
                ],
                &ATA_PROGRAM_PUBKEY,
            );
            let settlement_balance = self
                .rpc_client
                .get_token_account_balance_with_commitment(
                    &settlement_ata,
                    CommitmentConfig::confirmed(),
                )
                .await
                .map_err(|e| {
                    FacilitatorLocalError::ContractCall(format!(
                        "failed to get settlement account balance: {e}"
                    ))
                })?;
            let on_chain_balance: u64 = settlement_balance.value.amount.parse().unwrap_or(0);
            if on_chain_balance < required_amount {
                return Err(FacilitatorLocalError::InsufficientFunds(
                    MixedAddress::Solana(settlement_owner),
                ));
            }
        }

        let payer = payer_pubkey
            .map(SolanaAddress::from)
            .unwrap_or_else(|| SolanaAddress::from(self.keypair.pubkey()));

        tracing::info!(
            network = %self.network(),
            tx_signature = %sig,
            amount = total_credit,
            payer = %payer.pubkey,
            "Settlement account on-chain verification passed"
        );

        Ok(SettlementAccountVerifyResult {
            payer,
            tx_signature: sig,
        })
    }

    /// Settle a settlement account payment: verify on-chain, then sweep if needed.
    ///
    /// If `settle_secret_key` is provided, the facilitator sweeps USDC from the
    /// settlement account to `payTo` and closes the ATA.
    /// If not provided, returns the original transaction signature (funds already at payTo).
    ///
    /// Security (audit fix B1): replay protection via the global nonce store.
    /// The original on-chain `transaction_signature` is claimed once that transaction has
    /// verified, before anything is sent: a request that fails verification claims nothing,
    /// and a sweep that moved nothing gives the claim back. Subsequent calls with a claimed
    /// signature are rejected with an error.
    async fn settle_settlement_account(
        &self,
        payload: &crate::types::SettlementAccountPayload,
        requirements: &PaymentRequirements,
    ) -> Result<SettleResponse, FacilitatorLocalError> {
        // Step 0: Verify the on-chain transaction (bound to the settlement account when a
        // settleSecretKey is supplied) BEFORE claiming its signature, so a request that
        // fails here cannot hold a deposit away from the payment it belongs to.
        let verification = self
            .verify_settlement_account(payload, requirements)
            .await?;

        // Step 1: B1 REPLAY PROTECTION -- check and atomically mark the original tx signature.
        //
        // We key on (network, original_tx_signature) so the same on-chain transaction cannot
        // be settled twice across any facilitator instance.  The check-and-mark is atomic in
        // DynamoDB (conditional PutItem) and best-effort in the in-memory fallback. It still
        // comes before the sweep is sent, so two settles of one deposit sweep at most once.
        let network_str = self.network().to_string();
        let nonce_key = solana_settle_nonce_key(&network_str, &payload.transaction_signature);
        let nonce_store = get_solana_nonce_store().await;
        nonce_store
            .check_and_mark_used(&nonce_key, SOLANA_SETTLE_TTL_SECONDS)
            .await
            .map_err(|e| {
                tracing::warn!(
                    tx_sig = %payload.transaction_signature,
                    error = %e,
                    "Settlement account replay check rejected or store error"
                );
                FacilitatorLocalError::DecodingError(format!(
                    "settlement account replay protection: {e}"
                ))
            })?;

        // Step 2: If settleSecretKey is provided, sweep funds from settlement account to payTo.
        if let Some(ref secret_key_str) = payload.settle_secret_key {
            let swept = self
                .sweep_settlement_account(secret_key_str, payload, requirements, &verification)
                .await;
            // A sweep with no verdict may still land, so it keeps the claim. Every other
            // failure moved nothing -- refused before the send, refused by the node, or past
            // its blockhash (`send_and_confirm`) -- so the claim goes back and the same
            // deposit can be settled again. Best-effort: a failed release keeps it claimed.
            if let Err(ref error) = swept {
                if !matches!(error, FacilitatorLocalError::SettlementUnconfirmed(..)) {
                    if let Err(e) = nonce_store.release(&nonce_key).await {
                        tracing::warn!(
                            tx_sig = %payload.transaction_signature,
                            error = %e,
                            "Settlement account replay claim not released after a failed sweep"
                        );
                    }
                }
            }
            return swept;
        }

        // No settleSecretKey: the referenced transaction itself must have credited pay_to's ATA.
        // verify_settlement_account (audit 03) already enforced pay_to_credit >= required_amount
        // before we reach here, so returning the original tx signature is sound. This branch is
        // NO LONGER a blind "trust the client" path.
        tracing::info!(
            network = %self.network(),
            tx_signature = %verification.tx_signature,
            "Settlement account: no sweep needed, on-chain credit to pay_to already verified"
        );

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.clone().into(),
            transaction: Some(TransactionHash::Solana(
                *verification.tx_signature.as_array(),
            )),
            network: self.network(),
            proof_of_payment: None,
            extensions: None,
        })
    }

    /// Sweep USDC from a settlement account to payTo using the settlement secret key.
    ///
    /// Security (audit fix B1): the sweep moves exactly `max_amount_required`. A settlement
    /// account holding less is refused before anything is sent; extra funds stay in it.
    /// payTo's token account is created only by a sweep that closes the settlement account
    /// to the fee payer, so the fee payer never funds rent it does not get back.
    async fn sweep_settlement_account(
        &self,
        secret_key_str: &str,
        payload: &crate::types::SettlementAccountPayload,
        requirements: &PaymentRequirements,
        verification: &SettlementAccountVerifyResult,
    ) -> Result<SettleResponse, FacilitatorLocalError> {
        use solana_sdk::instruction::{AccountMeta, Instruction};
        use solana_sdk::message::Message;
        use solana_sdk::transaction::Transaction;

        // Decode the settlement keypair
        let settlement_keypair = settlement_keypair(secret_key_str)?;
        let settlement_pubkey = settlement_keypair.pubkey();

        // Parse asset mint and payTo
        let mint: Pubkey = match &requirements.asset {
            MixedAddress::Solana(pk) => *pk,
            _ => {
                return Err(FacilitatorLocalError::InvalidAddress(
                    "expected Solana asset address".to_string(),
                ))
            }
        };
        let pay_to: Pubkey = match &requirements.pay_to {
            MixedAddress::Solana(pk) => *pk,
            _ => {
                return Err(FacilitatorLocalError::InvalidAddress(
                    "expected Solana payTo address".to_string(),
                ))
            }
        };

        let token_program = spl_token::id();
        let fee_payer = self.keypair.pubkey();

        // Derive ATAs
        let (settlement_ata, _) = Pubkey::find_program_address(
            &[
                settlement_pubkey.as_ref(),
                token_program.as_ref(),
                mint.as_ref(),
            ],
            &ATA_PROGRAM_PUBKEY,
        );
        let (pay_to_ata, _) = Pubkey::find_program_address(
            &[pay_to.as_ref(), token_program.as_ref(), mint.as_ref()],
            &ATA_PROGRAM_PUBKEY,
        );

        // Parse required amount for the sweep cap.
        let required_amount: u64 = requirements
            .max_amount_required
            .0
            .to_string()
            .parse::<u64>()
            .map_err(|e| {
                FacilitatorLocalError::DecodingError(format!(
                    "cannot parse maxAmountRequired as u64 for sweep cap: {e}"
                ))
            })?;

        // Check settlement account ATA balance.
        let settlement_balance = self
            .rpc_client
            .get_token_account_balance(&settlement_ata)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!(
                    "failed to get settlement account balance: {e}"
                ))
            })?;
        let on_chain_balance: u64 = settlement_balance.amount.parse().unwrap_or(0);

        // B1 SWEEP CAP: never transfer more than required, even if the account holds extra.
        let sweep_amount = on_chain_balance.min(required_amount);

        if on_chain_balance == 0 {
            // SECURITY (audit 03): a settleSecretKey was supplied, which asserts funds are in the
            // settlement account awaiting a sweep. An empty settlement ATA means there is nothing
            // to move to pay_to. Returning success here previously forged a payment (an attacker
            // could supply a fresh empty keypair). Hard-error instead; the only sound "no sweep"
            // path is the settleSecretKey == None branch, which is guarded on-chain by the
            // pay_to_credit check in verify_settlement_account.
            return Err(FacilitatorLocalError::ContractCall(
                "settlement account ATA is empty: no funds to sweep to pay_to".to_string(),
            ));
        }

        // SECURITY: the sweep moves the price or nothing. Sweeping a smaller balance would
        // answer success for a payment pay_to did not receive in full, so it is refused here,
        // before any transaction is built (the balance can drop after verify: whoever holds
        // the settlement key can move it).
        if on_chain_balance < required_amount {
            return Err(FacilitatorLocalError::InsufficientFunds(
                MixedAddress::Solana(settlement_pubkey),
            ));
        }

        // The settlement ATA is closed (step 3) only when the whole balance moves.
        let closes_settlement = sweep_amount == on_chain_balance;

        // SECURITY: the fee payer pays transaction fees, never the rent of an account that
        // does not come back to it. payTo's token account is created (step 1) only when it
        // does not exist yet AND this transaction closes the settlement ATA with its rent
        // going to the fee payer, which pays the fee payer back for the new account. An
        // existing account gets no create instruction at all: if its owner closes it before
        // the sweep lands, the transfer fails instead of the fee payer funding a new one.
        let pay_to_ata_exists = self
            .rpc_client
            .get_account_with_commitment(&pay_to_ata, self.rpc_client.commitment())
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!(
                    "failed to read the payTo token account: {e}"
                ))
            })?
            .value
            .is_some_and(|account| account.owner == token_program);
        if !pay_to_ata_exists && !closes_settlement {
            return Err(FacilitatorLocalError::DecodingError(
                "settlement account: payTo has no token account for this mint, and the sweep \
                 creates one only when it closes the settlement account"
                    .to_string(),
            ));
        }

        tracing::info!(
            network = %self.network(),
            settlement_account = %settlement_pubkey,
            sweep_amount = sweep_amount,
            pay_to = %pay_to,
            "Sweeping settlement account to payTo"
        );

        let mut instructions = Vec::new();

        // 1. Create payTo ATA, only when it is missing and step 3 pays the rent back (above).
        if !pay_to_ata_exists {
            instructions.push(Instruction {
                program_id: ATA_PROGRAM_PUBKEY,
                accounts: vec![
                    AccountMeta::new(fee_payer, true),
                    AccountMeta::new(pay_to_ata, false),
                    AccountMeta::new_readonly(pay_to, false),
                    AccountMeta::new_readonly(mint, false),
                    AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                    AccountMeta::new_readonly(token_program, false),
                ],
                data: vec![1], // 1 = CreateIdempotent
            });
        }

        // 2. TransferChecked from settlement ATA to payTo ATA (capped at required_amount).
        instructions.push(
            spl_token::instruction::transfer_checked(
                &token_program,
                &settlement_ata,
                &mint,
                &pay_to_ata,
                &settlement_pubkey,
                &[],
                sweep_amount,
                6, // USDC decimals
            )
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!(
                    "failed to create transfer instruction: {e}"
                ))
            })?,
        );

        // 3. Close settlement ATA only if the full balance was swept.
        //
        // B1: When the sweep cap applies (on_chain_balance > required_amount), the account
        // retains the excess.  In this case we do NOT close the account because close_account
        // fails on a non-zero balance.  The excess remains in the settlement ATA and the
        // client is responsible for reclaiming it via a separate transaction.
        if closes_settlement {
            // The rent goes where the client asked, unless step 1 created payTo's account:
            // then it pays the fee payer back for that account.
            let rent_destination = if pay_to_ata_exists {
                payload
                    .settlement_rent_destination
                    .as_ref()
                    .and_then(|s| Pubkey::from_str(s).ok())
                    .unwrap_or(fee_payer)
            } else {
                fee_payer
            };

            instructions.push(
                spl_token::instruction::close_account(
                    &token_program,
                    &settlement_ata,
                    &rent_destination,
                    &settlement_pubkey,
                    &[],
                )
                .map_err(|e| {
                    FacilitatorLocalError::ContractCall(format!(
                        "failed to create close_account instruction: {e}"
                    ))
                })?,
            );
        } else {
            tracing::info!(
                network = %self.network(),
                on_chain_balance = on_chain_balance,
                sweep_amount = sweep_amount,
                excess = on_chain_balance - sweep_amount,
                "B1: sweep cap applied -- excess tokens remain in settlement ATA (not closed)"
            );
        }

        // Build and sign the transaction
        let recent_blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;

        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&fee_payer),
            &[&self.keypair, &settlement_keypair],
            recent_blockhash,
        );

        // Submit through the same path as an exact settle. Once the sweep is
        // handed over, a submission or status read that gets no answer is
        // reported unconfirmed under the sweep's own signature, never as a
        // refusal: the deposit has been marked used, so a caller told the
        // sweep failed would ask the buyer for another one.
        let tx_sig = TransactionInt::new(VersionedTransaction::from(tx))
            .send_and_confirm(
                &self.rpc_client,
                CommitmentConfig::confirmed(),
                self.network(),
            )
            .await
            .map_err(|e| match e {
                FacilitatorLocalError::ContractCall(e) => FacilitatorLocalError::ContractCall(
                    format!("settlement account sweep failed: {e}"),
                ),
                e => e,
            })?;

        tracing::info!(
            network = %self.network(),
            sweep_tx = %tx_sig,
            amount = sweep_amount,
            "Settlement account sweep successful"
        );

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.clone().into(),
            transaction: Some(TransactionHash::Solana(*tx_sig.as_array())),
            network: self.network(),
            proof_of_payment: None,
            extensions: None,
        })
    }
}

impl FromEnvByNetworkBuild for SolanaProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let env_var = from_env::rpc_env_name_from_network(network);
        let rpc_url = match std::env::var(env_var).ok() {
            Some(rpc_url) => rpc_url,
            None => {
                tracing::warn!(network=%network, "no RPC URL configured, skipping");
                return Ok(None);
            }
        };
        let keypair = from_env::SignerType::from_env()?.make_solana_wallet(network)?;
        let max_compute_unit_limit = Self::max_compute_unit_limit_from_env(network);
        let max_compute_unit_price = Self::max_compute_unit_price_from_env(network);
        let provider = SolanaProvider::try_new(
            keypair,
            rpc_url,
            network,
            max_compute_unit_limit,
            max_compute_unit_price,
        )?;
        Ok(Some(provider))
    }
}

pub struct VerifyTransferResult {
    pub payer: SolanaAddress,
    pub transaction: VersionedTransaction,
    /// The transfer was found in the simulation's inner instructions (a smart
    /// wallet's CPI), not as a top-level instruction of the transaction.
    pub transfer_in_cpi: bool,
}

pub struct SettlementAccountVerifyResult {
    pub payer: SolanaAddress,
    pub tx_signature: Signature,
}

#[derive(Debug)]
pub struct TransferCheckedInstruction {
    pub amount: u64,
    pub decimals: u8,
    pub source: Pubkey,
    pub mint: Pubkey,
    pub destination: Pubkey,
    pub authority: Pubkey,
    pub token_program: Pubkey,
    pub data: Vec<u8>,
}

impl NetworkProviderOps for SolanaProvider {
    fn signer_address(&self) -> MixedAddress {
        self.fee_payer()
    }

    fn network(&self) -> Network {
        self.chain.network
    }
}

impl Facilitator for SolanaProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        // B6: strict asset allow-list. Refuses arbitrary SPL mints before any
        // RPC call so a hostile payload cannot trick the facilitator into
        // settling a transfer for a token we did not pre-approve. Covers both
        // the standard transfer path and the settlement-account path.
        crate::chain::assert_supported_asset(
            self.network(),
            None,
            &request.payment_requirements.asset,
        )?;

        // Route: settlement account vs standard transaction
        if let ExactPaymentPayload::SolanaSettlementAccount(sa_payload) =
            &request.payment_payload.payload
        {
            // SECURITY (audit 03): kill-switch. The scheme is ON unless
            // ENABLE_SETTLEMENT_ACCOUNT is "false" or "0" (`is_settlement_account_enabled`).
            if !is_settlement_account_enabled() {
                return Err(FacilitatorLocalError::Other(
                    "settlement_account_disabled: ENABLE_SETTLEMENT_ACCOUNT=false".to_string(),
                ));
            }
            let result = self
                .verify_settlement_account(sa_payload, &request.payment_requirements)
                .await?;
            return Ok(VerifyResponse::valid(result.payer.into()));
        }

        let verification = self.verify_transfer(request).await?;
        self.verify_signed_by_its_signers(&verification.transaction)?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        // B6: strict asset allow-list. Mirrors the verify path so the
        // settlement-account flow (which bypasses verify_transfer) is also
        // gated.
        crate::chain::assert_supported_asset(
            self.network(),
            None,
            &request.payment_requirements.asset,
        )?;

        // Route: settlement account vs standard transaction
        if let ExactPaymentPayload::SolanaSettlementAccount(sa_payload) =
            &request.payment_payload.payload
        {
            // SECURITY (audit 03): kill-switch. The scheme is ON unless
            // ENABLE_SETTLEMENT_ACCOUNT is "false" or "0" (`is_settlement_account_enabled`).
            if !is_settlement_account_enabled() {
                return Err(FacilitatorLocalError::Other(
                    "settlement_account_disabled: ENABLE_SETTLEMENT_ACCOUNT=false".to_string(),
                ));
            }
            return self
                .settle_settlement_account(sa_payload, &request.payment_requirements)
                .await;
        }

        // Standard flow: verify + co-sign + submit
        let verification = self.verify_transfer(request).await?;
        let transfer_in_cpi = verification.transfer_in_cpi;

        tracing::info!(
            network = %self.network(),
            payer = %verification.payer.pubkey,
            num_instructions = verification.transaction.message.instructions().len(),
            "Processing Solana settlement"
        );

        // The transaction comes from the user exactly as they signed it.
        // User's signature is already in place (typically at index 1).
        // We just add our signature as fee payer (at index 0).
        let tx = TransactionInt::new(verification.transaction).sign(&self.keypair)?;

        // Verify all required signatures are present
        if !tx.is_fully_signed() {
            tracing::warn!(
                network = %self.network(),
                num_signatures = tx.inner.signatures.len(),
                num_required = tx.inner.message.header().num_required_signatures,
                "Transaction is not fully signed - missing user signature?"
            );
            return Ok(SettleResponse {
                success: false,
                error_reason: Some(FacilitatorErrorReason::UnexpectedSettleError),
                payer: verification.payer.into(),
                transaction: None,
                network: self.network(),
                proof_of_payment: None,
                extensions: None,
            });
        }

        // Submit the transaction to the network
        let tx_sig = tx
            .send_and_confirm(
                &self.rpc_client,
                CommitmentConfig::confirmed(),
                self.network(),
            )
            .await?;

        // A transfer found as a top-level instruction ran if the transaction
        // confirmed. A transfer found in the simulation's inner instructions is
        // up to the program that makes it, which can do something else once the
        // transaction lands: what it did is read back from the chain, and only
        // a credit of the price to payTo is a settlement.
        if transfer_in_cpi {
            match self
                .read_landed_payment(&tx_sig, &request.payment_requirements)
                .await
            {
                LandedPayment::Paid => {}
                LandedPayment::NotPaid(why) => {
                    tracing::warn!(
                        network = %self.network(),
                        tx_signature = %tx_sig,
                        payer = %verification.payer.pubkey,
                        reason = %why,
                        "Solana transaction confirmed without paying payTo; not a settlement"
                    );
                    return Ok(SettleResponse {
                        success: false,
                        error_reason: Some(FacilitatorErrorReason::FreeForm(
                            LANDED_WITHOUT_PAYMENT.to_string(),
                        )),
                        payer: verification.payer.into(),
                        transaction: None,
                        network: self.network(),
                        proof_of_payment: None,
                        extensions: None,
                    });
                }
                LandedPayment::Unknown(why) => {
                    tracing::warn!(
                        network = %self.network(),
                        tx_signature = %tx_sig,
                        reason = %why,
                        "Solana transaction confirmed; what it paid payTo is unknown"
                    );
                    return Err(FacilitatorLocalError::SettlementUnconfirmed(
                        TransactionHash::Solana(*tx_sig.as_array()),
                        self.network(),
                    ));
                }
            }
        }

        tracing::info!(
            network = %self.network(),
            tx_signature = %tx_sig,
            "Solana settlement successful"
        );

        let settle_response = SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.into(),
            transaction: Some(TransactionHash::Solana(*tx_sig.as_array())),
            network: self.network(),
            proof_of_payment: None, // ERC-8004 not supported on Solana yet
            extensions: None,
        };
        Ok(settle_response)
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            network_aliases: None,
            extra: Some(SupportedPaymentKindExtra {
                near_escrow: None,
                fee_payer: Some(self.signer_address()),
                tokens: Some(crate::network::exact_payment_tokens(self.network())),
                escrow: None,
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}

pub struct InstructionInt {
    instruction: CompiledInstruction,
    account_keys: Vec<Pubkey>,
}

impl InstructionInt {
    pub fn has_data(&self) -> bool {
        !self.instruction.data.is_empty()
    }

    pub fn has_accounts(&self) -> bool {
        !self.instruction.accounts.is_empty()
    }

    pub fn data(&self) -> Vec<u8> {
        self.instruction.data.clone()
    }

    pub fn data_slice(&self) -> &[u8] {
        self.instruction.data.as_slice()
    }

    pub fn assert_not_empty(&self) -> Result<(), FacilitatorLocalError> {
        if !self.has_data() || !self.has_accounts() {
            return Err(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_instructions".to_string(),
            ));
        }
        Ok(())
    }

    pub fn program_id(&self) -> Pubkey {
        *self.instruction.program_id(self.account_keys.as_slice())
    }

    pub fn account(&self, index: usize) -> Result<Pubkey, FacilitatorLocalError> {
        let account_index = self.instruction.accounts.get(index).cloned().ok_or(
            FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_instructions".to_string(),
            ),
        )?;
        let pubkey = self
            .account_keys
            .get(account_index as usize)
            .cloned()
            .ok_or(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_instructions".to_string(),
            ))?;
        Ok(pubkey)
    }
}

/// Whether a failed `sendTransaction` may still have put the transaction on its
/// way to a leader.
///
/// A node that answered with a JSON-RPC error refused it -- unless it says the
/// transaction was already processed -- and so did an
/// HTTP 4xx (a rate limit the client already retried, an auth failure). A
/// connection never made, a request never built, a transaction that failed to
/// serialize or to sign never left. Everything else -- a timeout, a dropped
/// connection, a 5xx, an answer that does not parse, a signature back that is
/// not ours -- can mean the RPC accepted it and the answer was lost.
fn send_may_have_landed(error: &solana_client::client_error::ClientError) -> bool {
    use solana_client::client_error::ClientErrorKind;
    use solana_client::rpc_request::RpcError;
    // A preflight that answers `AlreadyProcessed` is the Solana counterpart of
    // EVM's `already known`: this exact transaction was seen before, which is
    // not a refusal. It is read as "may have landed", never as a confirmed
    // success. HYPOTHESIS: the variant is the SDK's own, but the RPC's exact
    // wording was not measured against a live node; the text match
    // ("already been processed", the variant's `Display`) covers an answer
    // that carries no simulation data.
    let already_processed = error.get_transaction_error()
        == Some(solana_sdk::transaction::TransactionError::AlreadyProcessed)
        || error
            .to_string()
            .to_ascii_lowercase()
            .contains("already been processed");
    if already_processed {
        return true;
    }
    match error.kind() {
        ClientErrorKind::RpcError(RpcError::RpcResponseError { .. }) => false,
        ClientErrorKind::Reqwest(e) => {
            !(e.is_connect()
                || e.is_builder()
                || e.status().is_some_and(|status| status.is_client_error()))
        }
        // Only `serialize_and_encode` raises these before the request.
        ClientErrorKind::Custom(_)
        | ClientErrorKind::SigningError(_)
        | ClientErrorKind::TransactionError(_) => false,
        ClientErrorKind::Io(_)
        | ClientErrorKind::Middleware(_)
        | ClientErrorKind::SerdeJson(_)
        | ClientErrorKind::RpcError(_) => true,
    }
}

pub struct TransactionInt {
    inner: VersionedTransaction,
}

impl TransactionInt {
    pub fn new(transaction: VersionedTransaction) -> Self {
        Self { inner: transaction }
    }
    pub fn instruction(&self, index: usize) -> Result<InstructionInt, FacilitatorLocalError> {
        let instruction = self
            .inner
            .message
            .instructions()
            .get(index)
            .cloned()
            .ok_or(FacilitatorLocalError::DecodingError(
                "invalid_exact_svm_payload_transaction_instructions".to_string(),
            ))?;
        let account_keys = self.inner.message.static_account_keys().to_vec();

        Ok(InstructionInt {
            instruction,
            account_keys,
        })
    }

    pub fn is_fully_signed(&self) -> bool {
        let num_required = self.inner.message.header().num_required_signatures;
        if self.inner.signatures.len() < num_required as usize {
            return false;
        }
        let default = Signature::default();
        for signature in self.inner.signatures.iter() {
            if default.eq(signature) {
                return false;
            }
        }
        true
    }

    /// Sign the transaction as the fee payer.
    /// The user's signature should already be in place at the appropriate index.
    /// This function adds the facilitator signature at the fee payer's position (typically index 0).
    pub fn sign(self, keypair: &Keypair) -> Result<Self, FacilitatorLocalError> {
        let mut tx = self.inner.clone();
        let msg_bytes = tx.message.serialize();
        let signature = keypair
            .try_sign_message(msg_bytes.as_slice())
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;

        // Required signatures are the first N account keys
        let num_required = tx.message.header().num_required_signatures as usize;
        let static_keys = tx.message.static_account_keys();

        // Find signer's position in the account keys
        let pos = static_keys[..num_required]
            .iter()
            .position(|k| *k == keypair.pubkey())
            .ok_or_else(|| {
                tracing::error!(
                    fee_payer = %keypair.pubkey(),
                    num_required = num_required,
                    account_keys = ?static_keys[..num_required].iter().map(|k| k.to_string()).collect::<Vec<_>>(),
                    "Fee payer not found in transaction's required signers"
                );
                FacilitatorLocalError::DecodingError(
                    "fee_payer_not_in_transaction_signers".to_string(),
                )
            })?;

        // Ensure signature vector is large enough, then place the signature
        if tx.signatures.len() < num_required {
            tx.signatures.resize(num_required, Signature::default());
        }

        // Log signature placement for debugging
        let default_sig = Signature::default();
        let existing_sigs: Vec<_> = tx
            .signatures
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if *s == default_sig {
                    format!("[{}]: empty", i)
                } else {
                    format!("[{}]: present", i)
                }
            })
            .collect();
        tracing::debug!(
            fee_payer_position = pos,
            existing_signatures = ?existing_sigs,
            "Adding fee payer signature"
        );

        tx.signatures[pos] = signature;
        Ok(Self { inner: tx })
    }

    pub async fn send(&self, rpc_client: &RpcClient) -> Result<Signature, FacilitatorLocalError> {
        self.submit(rpc_client)
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))
    }

    /// `sendTransaction`, with the client's own error kept so the caller can
    /// tell a node's refusal from a submission whose outcome is unknown.
    // The client's error type is its own; boxing it would only move the
    // allocation to the one caller that inspects it.
    #[allow(clippy::result_large_err)]
    async fn submit(
        &self,
        rpc_client: &RpcClient,
    ) -> solana_client::client_error::Result<Signature> {
        // Preflight is ON by default, and that is a deliberate reversal.
        //
        // With `skip_preflight: true` the RPC validates nothing -- not the
        // blockhash, not the signatures -- and hands back a signature
        // regardless. A transaction that can never land looks identical to one
        // that will: you get a signature, then thirty seconds of silence, then
        // a timeout naming a signature that does not exist on chain. Measured
        // on 2026-08-18: 10 such timeouts against 2 successful Solana
        // settlements in 24h, with a healthy premium RPC.
        //
        // With preflight on, the same failure comes back immediately and says
        // what it was ("blockhash not found", "signature verification failure"),
        // which is the difference between a diagnosable error and a mystery.
        //
        // `SOLANA_SKIP_PREFLIGHT=true` restores the old behaviour for anyone who
        // has already simulated and wants the round trip back.
        let skip_preflight = std::env::var("SOLANA_SKIP_PREFLIGHT")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        rpc_client
            .send_transaction_with_config(
                &self.inner,
                RpcSendTransactionConfig {
                    skip_preflight,
                    // The RPC keeps re-forwarding to upcoming leaders. Under
                    // congestion five attempts inside a ~60s blockhash window is
                    // thin; 20 costs nothing when the transaction lands early.
                    max_retries: Some(20),
                    ..RpcSendTransactionConfig::default()
                },
            )
            .await
    }

    pub async fn send_and_confirm(
        &self,
        rpc_client: &RpcClient,
        commitment_config: CommitmentConfig,
        network: Network,
    ) -> Result<Signature, FacilitatorLocalError> {
        // The id of this transaction is its first signature, known before it
        // is sent. A submission that fails without the node refusing it may
        // still reach a leader, so it is reported as unconfirmed under that id
        // rather than as a failure the caller would retry.
        let tx_sig = match self.submit(rpc_client).await {
            Ok(tx_sig) => tx_sig,
            Err(e) => {
                let id = self.inner.signatures.first().copied();
                return Err(match id.filter(|_| send_may_have_landed(&e)) {
                    Some(id) => {
                        tracing::warn!(
                            tx_sig = %id,
                            error = %crate::redact::scrub_urls(&e.to_string()),
                            "Solana submission outcome unknown; the transaction may still land"
                        );
                        FacilitatorLocalError::SettlementUnconfirmed(
                            TransactionHash::Solana(*id.as_array()),
                            network,
                        )
                    }
                    None => FacilitatorLocalError::ContractCall(format!("{e}")),
                });
            }
        };

        // Timeout for confirmation - configurable via environment variable
        // Default: 30 seconds (Solana blocks are ~400ms, 30s = ~75 blocks)
        // 90s, not 30. A Solana blockhash stays valid for ~150 slots (~60-90s),
        // so giving up at 30 abandons transactions that can still land -- and
        // the caller is told the payment failed while it is still in flight.
        // Waiting out the blockhash means a timeout now genuinely implies the
        // transaction can never confirm.
        let timeout_secs = std::env::var("SOLANA_CONFIRM_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(90);
        let timeout_duration = Duration::from_secs(timeout_secs);

        let confirmation_future = async {
            loop {
                // The transaction was accepted: a status read that fails says
                // nothing about its fate, so the signature travels with the
                // error instead of a failure the caller would retry.
                let confirmed = rpc_client
                    .confirm_transaction_with_commitment(&tx_sig, commitment_config)
                    .await
                    .map_err(|e| {
                        tracing::warn!(
                            tx_sig = %tx_sig,
                            error = %crate::redact::scrub_urls(&e.to_string()),
                            "Solana status read failed; the transaction may still confirm"
                        );
                        FacilitatorLocalError::SettlementUnconfirmed(
                            TransactionHash::Solana(*tx_sig.as_array()),
                            network,
                        )
                    })?;
                if confirmed.value {
                    return Ok::<Signature, FacilitatorLocalError>(tx_sig);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };

        match tokio::time::timeout(timeout_duration, confirmation_future).await {
            Ok(result) => result,
            Err(_) => {
                // Before calling it a failure, ask the chain one more time
                // whether it landed.
                //
                // "May have been submitted" is the worst thing to tell a
                // caller: the money may have moved while the seller is being
                // told the payment failed. One last status read turns that into
                // a definite answer whenever the chain has one -- the same
                // discipline as checking `receipt.status()` on EVM rather than
                // assuming a submitted transaction succeeded.
                match rpc_client
                    .confirm_transaction_with_commitment(&tx_sig, commitment_config)
                    .await
                {
                    Ok(c) if c.value => {
                        tracing::info!(
                            tx_sig = %tx_sig,
                            "Transaction confirmed on the final check after the timeout window"
                        );
                        return Ok(tx_sig);
                    }
                    Ok(_) => {
                        tracing::warn!(
                            tx_sig = %tx_sig,
                            timeout_secs,
                            "Solana transaction never confirmed; the blockhash window has passed so it cannot land"
                        );
                        Err(FacilitatorLocalError::ContractCall(format!(
                            "Solana transaction {tx_sig} did not confirm within {timeout_secs}s. \
                             The blockhash window has passed, so it can no longer land and the \
                             payment did NOT settle. Retry with a fresh blockhash."
                        )))
                    }
                    Err(e) => {
                        // We genuinely do not know. Say so rather than
                        // asserting a failure that may not have happened.
                        //
                        // Note the asymmetry with the `Ok(_)` arm above: there
                        // the chain answered and the blockhash window has
                        // passed, so the transaction can never land and a
                        // retry is the correct advice. Here the chain gave no
                        // answer at all, so this is the unconfirmed case and
                        // the signature has to travel with the error -- the
                        // prose already told the caller to "check the
                        // signature on chain", in a string only a human can
                        // parse.
                        tracing::warn!(
                            tx_sig = %tx_sig,
                            timeout_secs,
                            error = %crate::redact::scrub_urls(&e.to_string()),
                            "Could not determine the fate of a Solana transaction after the timeout"
                        );
                        Err(FacilitatorLocalError::SettlementUnconfirmed(
                            TransactionHash::Solana(*tx_sig.as_array()),
                            network,
                        ))
                    }
                }
            }
        }
    }

    #[allow(dead_code)] // Public for consumption by downstream crates.
    pub fn as_base64(&self) -> Result<String, FacilitatorLocalError> {
        let bytes = bincode::serialize(&self.inner)
            .map_err(|e| FacilitatorLocalError::DecodingError(format!("{e}")))?;
        let base64_bytes = Base64Bytes::encode(bytes);
        let string = String::from_utf8(base64_bytes.0.into_owned())
            .map_err(|e| FacilitatorLocalError::DecodingError(format!("{e}")))?;
        Ok(string)
    }
}

// ============================================================================
// Tests for security invariants (audit fixes B1 and F1)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use solana_client::rpc_request::RpcRequest;
    use solana_sdk::instruction::Instruction;
    use solana_sdk::message::Message;
    use solana_sdk::transaction::Transaction;
    use spl_token_2022::extension::{BaseStateWithExtensionsMut, StateWithExtensionsMut};

    // ---- Token-2022 verify: PYUSD allow-list and the mint's transfer fee ------
    //
    // These drive `Facilitator::verify` end to end against a mock RPC. The mint
    // accounts are the REAL on-chain bytes (getAccountInfo, base64, finalized,
    // 2026-09-13), not hand-built vectors: the transfer-fee guard has to parse
    // what PayPal and Agora actually deployed.

    const PYUSD_MAINNET_MINT: &str = "2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo";
    const AUSD_MAINNET_MINT: &str = "AUSD1jCcCyPLybk1YnvPWsHQSrZ46dxwoMniN4N2UEB9";

    /// PYUSD mint account, Solana mainnet slot 446743095 (866 bytes).
    /// transferFeeConfig: 0 bps, maximumFee 0, both entries from epoch 605.
    const PYUSD_MAINNET_MINT_B64: &str = "AQAAAGyRqkllkBL4q+lh7CS2EHSSZUdTL/CU7VtpOYLbmHMTPvSEM5CDAgAGAQEAAAAXhTJh72q4Uypn8FOGWq0xKT/PB88SCrW5oVcGVI3AKwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQMAIAAXhTJh72q4Uypn8FOGWq0xKT/PB88SCrW5oVcGVI3AKwwAIAAXhTJh72q4Uypn8FOGWq0xKT/PB88SCrW5oVcGVI3AKwEAbAAXhTJh72q4Uypn8FOGWq0xKT/PB88SCrW5oVcGVI3AKxeFMmHvarhTKmfwU4ZarTEpP88HzxIKtbmhVwZUjcArAAAAAAAAAABdAgAAAAAAAAAAAAAAAAAAAABdAgAAAAAAAAAAAAAAAAAAAAAEAEEAF4UyYe9quFMqZ/BThlqtMSk/zwfPEgq1uaFXBlSNwCsAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQAIEAF4UyYe9quFMqZ/BThlqtMSk/zwfPEgq1uaFXBlSNwCscN+ZDO3ME3YJzeuQNm4vzxJ9bDmxJqNUzKLPlBpAcVwEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADgBAABeFMmHvarhTKmfwU4ZarTEpP88HzxIKtbmhVwZUjcArAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAASAEAAF4UyYe9quFMqZ/BThlqtMSk/zwfPEgq1uaFXBlSNwCsXkkg7bIoqh7dHHYFPlZH5OVyECpzj2fTVun06S4p0nhMArgAXhTJh72q4Uypn8FOGWq0xKT/PB88SCrW5oVcGVI3AKxeSSDtsiiqHt0cdgU+Vkfk5XIQKnOPZ9NW6fTpLinSeCgAAAFBheVBhbCBVU0QFAAAAUFlVU0RPAAAAaHR0cHM6Ly90b2tlbi1tZXRhZGF0YS5wYXhvcy5jb20vcHl1c2RfbWV0YWRhdGEvcHJvZC9zb2xhbmEvcHl1c2RfbWV0YWRhdGEuanNvbgAAAAA=";

    /// AUSD mint account, Solana mainnet slot 446743098 (895 bytes).
    /// transferFeeConfig: 0 bps, maximumFee 1_000_000, both entries from epoch 724.
    const AUSD_MAINNET_MINT_B64: &str = "AQAAAMOSBBNIrZ30q7bS6nks2GZ1Q7trvbXm+Ae5jxjAjDYeqqotl/0CAAAGAQEAAABQUymI/Gx1HsDmGa2p2RwvDgQNrShd0PcHdNXT6cY8rAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQMAIACzSF9xVg5DV9jy3vEg0xhShcLif3Pkz+feC9nbTgP/GAwAIABObi70qCWGj5qOj0PTsRGsNjy+H3yfox5TL518nnDsZwEAbACzSF9xVg5DV9jy3vEg0xhShcLif3Pkz+feC9nbTgP/GLNIX3FWDkNX2PLe8SDTGFKFwuJ/c+TP594L2dtOA/8YAAAAAAAAAADUAgAAAAAAAEBCDwAAAAAAAADUAgAAAAAAAEBCDwAAAAAAAAAEAEEAs0hfcVYOQ1fY8t7xINMYUoXC4n9z5M/n3gvZ204D/xgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAQAIEAs0hfcVYOQ1fY8t7xINMYUoXC4n9z5M/n3gvZ204D/xhY0WVHP5Y/c7RO15LRwgarfUfESJ93BmP4wsPhSxp/KQEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAADgBAALNIX3FWDkNX2PLe8SDTGFKFwuJ/c+TP594L2dtOA/8YAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAASAEAAs0hfcVYOQ1fY8t7xINMYUoXC4n9z5M/n3gvZ204D/xiMwF2vQ0qn90haKEhPwwaTEThLF8NttR9LO7ICy8gMyBMAywCzSF9xVg5DV9jy3vEg0xhShcLif3Pkz+feC9nbTgP/GIzAXa9DSqf3SFooSE/DBpMROEsXw221H0s7sgLLyAzIBAAAAEFVU0QEAAAAQVVTRHMAAABodHRwczovL3Jhdy5naXRodWJ1c2VyY29udGVudC5jb20vYWdvcmEtZmluYW5jZS9wdWJsaWMtYXNzZXRzL3JlZnMvaGVhZHMvbWFzdGVyL3NyYy90b2tlbi1tZXRhZGF0YS9zb2xhbmEtYXVzZC5qc29uAAAAAA==";

    /// Mainnet epoch when the fixtures were captured.
    const FIXTURE_EPOCH: u64 = 1034;

    fn b64(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn unb64(data: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap()
    }

    fn mock_provider(
        network: Network,
        mocks: Vec<(RpcRequest, serde_json::Value)>,
    ) -> SolanaProvider {
        SolanaProvider {
            keypair: Arc::new(Keypair::new()),
            chain: SolanaChain::try_from(network).unwrap(),
            rpc_client: Arc::new(RpcClient::new_mock_with_mocks_map(
                "succeeds",
                mocks.into_iter().collect(),
            )),
            max_compute_unit_limit: 400_000,
            max_compute_unit_price: 1_000_000,
        }
    }

    fn ui_account(owner: &Pubkey, data: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "lamports": 2_039_280u64,
            "data": [b64(data), "base64"],
            "owner": owner.to_string(),
            "executable": false,
            "rentEpoch": 0,
            "space": data.len(),
        })
    }

    fn multiple_accounts(accounts: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "context": { "slot": 1 }, "value": accounts })
    }

    fn clock_account(epoch: u64) -> serde_json::Value {
        let clock = Clock {
            slot: 1,
            epoch_start_timestamp: 0,
            epoch,
            leader_schedule_epoch: epoch + 1,
            unix_timestamp: 0,
        };
        ui_account(
            &solana_sdk::sysvar::ID,
            &bincode::serialize(&clock).unwrap(),
        )
    }

    /// The deployed mint with only its newer transfer fee rewritten.
    fn mint_with_newer_transfer_fee(
        mint_b64: &str,
        epoch: u64,
        basis_points: u16,
        maximum_fee: u64,
    ) -> Vec<u8> {
        let mut data = unb64(mint_b64);
        {
            let mut mint =
                StateWithExtensionsMut::<spl_token_2022::state::Mint>::unpack(&mut data).unwrap();
            let config = mint.get_extension_mut::<TransferFeeConfig>().unwrap();
            config.newer_transfer_fee.epoch = epoch.into();
            config.newer_transfer_fee.transfer_fee_basis_points = basis_points.into();
            config.newer_transfer_fee.maximum_fee = maximum_fee.into();
        }
        data
    }

    /// A payment the way a standard wallet builds it: compute budget, then a
    /// top-level Token-2022 TransferChecked into pay_to's ATA, signed by the
    /// payer and left for the fee payer to sign.
    fn token_2022_payment(
        fee_payer: &Pubkey,
        payer_keypair: &Keypair,
        mint: &Pubkey,
        pay_to: &Pubkey,
        amount: u64,
    ) -> String {
        let payer = &payer_keypair.pubkey();
        let ata = |owner: &Pubkey| {
            Pubkey::find_program_address(
                &[owner.as_ref(), spl_token_2022::ID.as_ref(), mint.as_ref()],
                &ATA_PROGRAM_PUBKEY,
            )
            .0
        };
        let mut limit = vec![2u8];
        limit.extend_from_slice(&200_000u32.to_le_bytes());
        let mut price = vec![3u8];
        price.extend_from_slice(&1u64.to_le_bytes());
        let instructions = [
            Instruction {
                program_id: solana_sdk::compute_budget::ID,
                accounts: vec![],
                data: limit,
            },
            Instruction {
                program_id: solana_sdk::compute_budget::ID,
                accounts: vec![],
                data: price,
            },
            spl_token_2022::instruction::transfer_checked(
                &spl_token_2022::ID,
                &ata(payer),
                mint,
                &ata(pay_to),
                payer,
                &[],
                amount,
                6,
            )
            .unwrap(),
        ];
        let mut tx = Transaction::new_unsigned(Message::new(&instructions, Some(fee_payer)));
        tx.partial_sign(&[payer_keypair], solana_sdk::hash::Hash::default());
        b64(&bincode::serialize(&VersionedTransaction::from(tx)).unwrap())
    }

    fn verify_request(
        network: Network,
        asset: &Pubkey,
        pay_to: &Pubkey,
        amount: u64,
        transaction: String,
    ) -> VerifyRequest {
        serde_json::from_value(serde_json::json!({
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1,
                "scheme": "exact",
                "network": network.to_string(),
                "payload": { "transaction": transaction },
            },
            "paymentRequirements": {
                "scheme": "exact",
                "network": network.to_string(),
                "maxAmountRequired": amount.to_string(),
                "resource": "https://example.com/paid",
                "description": "",
                "mimeType": "application/json",
                "payTo": pay_to.to_string(),
                "maxTimeoutSeconds": 60,
                "asset": asset.to_string(),
            },
        }))
        .unwrap()
    }

    /// Mocks for a verify that reaches the transfer-fee guard: first the source
    /// and destination ATAs exist, then the mint and the clock.
    fn mocks_through_fee_guard(
        mint_data: &[u8],
        epoch: u64,
    ) -> Vec<(RpcRequest, serde_json::Value)> {
        let token_account = ui_account(&spl_token_2022::ID, &[0u8; 82]);
        vec![
            (
                RpcRequest::GetMultipleAccounts,
                multiple_accounts(vec![token_account.clone(), token_account]),
            ),
            (
                RpcRequest::GetMultipleAccounts,
                multiple_accounts(vec![
                    ui_account(&spl_token_2022::ID, mint_data),
                    clock_account(epoch),
                ]),
            ),
        ]
    }

    #[tokio::test]
    async fn test_verify_pyusd_token2022_transfer_listed_mint_passes_unlisted_mint_fails() {
        let pyusd = Pubkey::from_str(PYUSD_MAINNET_MINT).unwrap();
        let payer = Keypair::new();
        let pay_to = Keypair::new().pubkey();
        let amount = 10_000; // 0.01 PYUSD

        let provider = mock_provider(
            Network::Solana,
            mocks_through_fee_guard(&unb64(PYUSD_MAINNET_MINT_B64), FIXTURE_EPOCH),
        );
        let tx = token_2022_payment(&provider.keypair.pubkey(), &payer, &pyusd, &pay_to, amount);
        let result = provider
            .verify(&verify_request(
                Network::Solana,
                &pyusd,
                &pay_to,
                amount,
                tx,
            ))
            .await;
        assert!(
            result.is_ok(),
            "a Token-2022 TransferChecked of listed PYUSD must verify: {result:?}"
        );

        // The same transaction shape over a Token-2022 mint nobody listed.
        let unlisted = Keypair::new().pubkey();
        let provider = mock_provider(Network::Solana, vec![]);
        let tx = token_2022_payment(
            &provider.keypair.pubkey(),
            &payer,
            &unlisted,
            &pay_to,
            amount,
        );
        let err = provider
            .verify(&verify_request(
                Network::Solana,
                &unlisted,
                &pay_to,
                amount,
                tx,
            ))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unsupported_asset"), "{err}");
    }

    #[tokio::test]
    async fn test_verify_rejects_token2022_mint_with_nonzero_transfer_fee() {
        let payer = Keypair::new();
        let pay_to = Keypair::new().pubkey();
        let amount = 10_000;
        let as_deployed = |mint_b64: &str| unb64(mint_b64);

        // (case, mint address, mint account bytes, must verify)
        //
        // AUSD goes first: it was already listed before PYUSD, so its fee case is
        // the one that fails for the right reason on a facilitator without the guard.
        let cases = [
            (
                "AUSD as deployed: 0 bps under a 1 AUSD cap is still 0",
                AUSD_MAINNET_MINT,
                as_deployed(AUSD_MAINNET_MINT_B64),
                true,
            ),
            (
                "AUSD with a 1 bps fee in force",
                AUSD_MAINNET_MINT,
                mint_with_newer_transfer_fee(AUSD_MAINNET_MINT_B64, FIXTURE_EPOCH, 1, 1_000_000),
                false,
            ),
            (
                "PYUSD as deployed",
                PYUSD_MAINNET_MINT,
                as_deployed(PYUSD_MAINNET_MINT_B64),
                true,
            ),
            (
                "PYUSD with a 0.5% fee in force",
                PYUSD_MAINNET_MINT,
                mint_with_newer_transfer_fee(PYUSD_MAINNET_MINT_B64, FIXTURE_EPOCH, 50, u64::MAX),
                false,
            ),
            (
                "PYUSD with a 0.5% fee scheduled two epochs out",
                PYUSD_MAINNET_MINT,
                mint_with_newer_transfer_fee(
                    PYUSD_MAINNET_MINT_B64,
                    FIXTURE_EPOCH + 2,
                    50,
                    u64::MAX,
                ),
                false,
            ),
        ];

        for (case, mint, mint_data, must_verify) in cases {
            let mint = Pubkey::from_str(mint).unwrap();
            let provider = mock_provider(
                Network::Solana,
                mocks_through_fee_guard(&mint_data, FIXTURE_EPOCH),
            );
            let tx = token_2022_payment(&provider.keypair.pubkey(), &payer, &mint, &pay_to, amount);
            let result = provider
                .verify(&verify_request(Network::Solana, &mint, &pay_to, amount, tx))
                .await;
            if must_verify {
                assert!(result.is_ok(), "{case}: {result:?}");
            } else {
                let err = result.expect_err(case);
                assert!(
                    err.to_string().contains("transfer_fee_not_zero"),
                    "{case}: {err}"
                );
            }
        }
    }

    /// `transaction` with the signature at `signer`'s position replaced.
    fn with_signature_of(transaction: &str, signer: &Pubkey, signature: Signature) -> String {
        let mut tx: VersionedTransaction = bincode::deserialize(&unb64(transaction)).unwrap();
        let position = tx
            .message
            .static_account_keys()
            .iter()
            .position(|key| key == signer)
            .unwrap();
        tx.signatures[position] = signature;
        b64(&bincode::serialize(&tx).unwrap())
    }

    /// A payment verifies only when every signer it names, other than the fee
    /// payer settle signs for, has signed it: without the payer's signature, or
    /// with one another key made, it is refused; signed by the payer, it verifies.
    #[tokio::test]
    async fn test_verify_requires_every_signer_but_the_fee_payer_to_have_signed() {
        let pyusd = Pubkey::from_str(PYUSD_MAINNET_MINT).unwrap();
        let payer = Keypair::new();
        let pay_to = Keypair::new().pubkey();
        let amount = 10_000;

        /// What sits at the payer's signature position.
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum PayerSignature {
            Own,
            Missing,
            AnotherKeys,
        }
        for case in [
            PayerSignature::Own,
            PayerSignature::Missing,
            PayerSignature::AnotherKeys,
        ] {
            let provider = mock_provider(
                Network::Solana,
                mocks_through_fee_guard(&unb64(PYUSD_MAINNET_MINT_B64), FIXTURE_EPOCH),
            );
            let signed =
                token_2022_payment(&provider.keypair.pubkey(), &payer, &pyusd, &pay_to, amount);
            let message = bincode::deserialize::<VersionedTransaction>(&unb64(&signed))
                .unwrap()
                .message
                .serialize();
            let tx = match case {
                PayerSignature::Own => signed,
                PayerSignature::Missing => {
                    with_signature_of(&signed, &payer.pubkey(), Signature::default())
                }
                PayerSignature::AnotherKeys => with_signature_of(
                    &signed,
                    &payer.pubkey(),
                    Keypair::new().sign_message(&message),
                ),
            };
            let result = provider
                .verify(&verify_request(
                    Network::Solana,
                    &pyusd,
                    &pay_to,
                    amount,
                    tx,
                ))
                .await;
            if case == PayerSignature::Own {
                assert!(result.is_ok(), "{case:?}: {result:?}");
            } else {
                let err = result.expect_err(&format!("{case:?}"));
                assert!(err.to_string().contains("signature"), "{case:?}: {err}");
            }
        }
    }

    #[test]
    fn test_transfer_fee_upper_bound_on_the_deployed_mints() {
        let fee = |owner: &Pubkey, data: &[u8], epoch: u64| {
            transfer_fee_upper_bound(owner, data, epoch, 10_000)
        };
        let t22 = spl_token_2022::ID;

        // As deployed, neither mint takes anything.
        assert_eq!(
            fee(&t22, &unb64(PYUSD_MAINNET_MINT_B64), FIXTURE_EPOCH).unwrap(),
            0
        );
        assert_eq!(
            fee(&t22, &unb64(AUSD_MAINNET_MINT_B64), FIXTURE_EPOCH).unwrap(),
            0
        );

        // 0.5% in force: 50 units out of 10_000.
        let in_force =
            mint_with_newer_transfer_fee(PYUSD_MAINNET_MINT_B64, FIXTURE_EPOCH, 50, u64::MAX);
        assert_eq!(fee(&t22, &in_force, FIXTURE_EPOCH).unwrap(), 50);

        // Scheduled two epochs out: already counted, so it cannot race a settlement.
        let scheduled =
            mint_with_newer_transfer_fee(PYUSD_MAINNET_MINT_B64, FIXTURE_EPOCH + 2, 50, u64::MAX);
        assert_eq!(fee(&t22, &scheduled, FIXTURE_EPOCH).unwrap(), 50);

        // maximumFee caps the fee: 50 bps of 10_000 capped at 3.
        let capped = mint_with_newer_transfer_fee(PYUSD_MAINNET_MINT_B64, FIXTURE_EPOCH, 50, 3);
        assert_eq!(fee(&t22, &capped, FIXTURE_EPOCH).unwrap(), 3);

        // A Token-2022 mint with no extensions (base layout only): no fee config, no fee.
        let mut plain = [0u8; 82];
        plain[44] = 6; // decimals
        plain[45] = 1; // is_initialized
        assert_eq!(fee(&t22, &plain, FIXTURE_EPOCH).unwrap(), 0);

        // Legacy SPL Token (USDC) never has a fee.
        assert_eq!(fee(&spl_token::ID, &plain, FIXTURE_EPOCH).unwrap(), 0);

        // Not a token program: refused, not read as fee-free.
        let err = fee(
            &solana_sdk::system_program::ID,
            &unb64(PYUSD_MAINNET_MINT_B64),
            FIXTURE_EPOCH,
        )
        .unwrap_err();
        assert!(err.to_string().contains("mint_not_a_token_mint"), "{err}");

        // Truncated account data: refused.
        let truncated = &unb64(PYUSD_MAINNET_MINT_B64)[..100];
        assert!(fee(&t22, truncated, FIXTURE_EPOCH).is_err());
    }

    // ---- parse_token_account_balance ----------------------------------------

    #[test]
    fn test_parse_token_account_balance_valid() {
        // Build a 165-byte buffer with amount=1_000_000 at bytes 64..72.
        let mut buf = vec![0u8; 165];
        let amount: u64 = 1_000_000;
        buf[64..72].copy_from_slice(&amount.to_le_bytes());
        let result = parse_token_account_balance(&buf);
        assert_eq!(result, Some(1_000_000u64));
    }

    #[test]
    fn test_parse_token_account_balance_zero() {
        let buf = vec![0u8; 165];
        let result = parse_token_account_balance(&buf);
        assert_eq!(result, Some(0u64));
    }

    #[test]
    fn test_parse_token_account_balance_too_short() {
        // Buffer shorter than 72 bytes should return None.
        let buf = vec![0u8; 71];
        let result = parse_token_account_balance(&buf);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_token_account_balance_max() {
        let mut buf = vec![0u8; 165];
        let amount = u64::MAX;
        buf[64..72].copy_from_slice(&amount.to_le_bytes());
        let result = parse_token_account_balance(&buf);
        assert_eq!(result, Some(u64::MAX));
    }

    // ---- solana_settle_nonce_key --------------------------------------------

    #[test]
    fn test_solana_settle_nonce_key_format() {
        let key = solana_settle_nonce_key("solana", "5Kx9abcdef1234567890ABCDEF");
        assert_eq!(key, "solana-settle#solana#5Kx9abcdef1234567890ABCDEF");
        // Verify it starts with the right prefix (used for DynamoDB chain field extraction).
        assert!(key.starts_with("solana-settle#"));
    }

    #[test]
    fn test_solana_settle_nonce_key_devnet() {
        let key = solana_settle_nonce_key("solana-devnet", "TXSIG");
        assert_eq!(key, "solana-settle#solana-devnet#TXSIG");
    }

    // ---- B1 sweep cap invariant (unit-level) --------------------------------
    //
    // The sweep cap logic: sweep_amount = on_chain_balance.min(required_amount)
    // This is a pure arithmetic invariant; we test the formula directly.

    #[test]
    fn test_sweep_cap_balance_exceeds_required() {
        let on_chain_balance: u64 = 2_000_000;
        let required_amount: u64 = 1_000_000;
        let sweep_amount = on_chain_balance.min(required_amount);
        assert_eq!(sweep_amount, 1_000_000, "must cap at required_amount");
        assert!(sweep_amount < on_chain_balance, "must not drain excess");
    }

    #[test]
    fn test_sweep_cap_balance_equals_required() {
        let on_chain_balance: u64 = 1_000_000;
        let required_amount: u64 = 1_000_000;
        let sweep_amount = on_chain_balance.min(required_amount);
        assert_eq!(sweep_amount, 1_000_000);
        // When equal, close_account should be included (sweep_amount == on_chain_balance).
        assert_eq!(sweep_amount, on_chain_balance);
    }

    // A balance below the price is never swept: see
    // `sweep_outcome_tests::a_settlement_account_below_the_price_is_refused_before_anything_is_sent`.

    // ---- B1 nonce store replay protection (integration with MemoryNonceStore) --

    #[tokio::test]
    async fn test_settlement_replay_protection_memory_store() {
        use crate::nonce_store::MemoryNonceStore;

        let store = MemoryNonceStore::new();
        let key = solana_settle_nonce_key("solana", "FAKE_TX_SIG_1234");

        // First settle should succeed.
        assert!(
            store
                .check_and_mark_used(&key, SOLANA_SETTLE_TTL_SECONDS)
                .await
                .is_ok(),
            "first settle must succeed"
        );

        // Second settle with same signature must be rejected.
        let result = store
            .check_and_mark_used(&key, SOLANA_SETTLE_TTL_SECONDS)
            .await;
        assert!(
            matches!(
                result,
                Err(crate::nonce_store::NonceStoreError::NonceAlreadyUsed(_))
            ),
            "second settle with same tx_sig must be rejected as replay"
        );
    }

    #[tokio::test]
    async fn test_settlement_replay_protection_different_sigs() {
        use crate::nonce_store::MemoryNonceStore;

        let store = MemoryNonceStore::new();
        let key1 = solana_settle_nonce_key("solana", "TX_SIG_A");
        let key2 = solana_settle_nonce_key("solana", "TX_SIG_B");

        // Two different signatures should both be accepted independently.
        assert!(store
            .check_and_mark_used(&key1, SOLANA_SETTLE_TTL_SECONDS)
            .await
            .is_ok());
        assert!(store
            .check_and_mark_used(&key2, SOLANA_SETTLE_TTL_SECONDS)
            .await
            .is_ok());
        // Now key1 replay is rejected.
        assert!(store
            .check_and_mark_used(&key1, SOLANA_SETTLE_TTL_SECONDS)
            .await
            .is_err());
    }

    // ---- F1 post-balance delta logic ----------------------------------------
    //
    // Test the logical invariants of the balance delta check without constructing
    // a full SolanaProvider (which requires a live RPC and keypair).

    #[test]
    fn test_balance_delta_exact_match() {
        let pre: Option<u64> = Some(0);
        let post: Option<u64> = Some(1_000_000);
        let required: u64 = 1_000_000;
        let delta = post.unwrap_or(0).saturating_sub(pre.unwrap_or(0));
        assert_eq!(
            delta, required,
            "delta must equal required when ATA starts at 0"
        );
    }

    #[test]
    fn test_balance_delta_with_existing_balance() {
        let pre: Option<u64> = Some(500_000);
        let post: Option<u64> = Some(1_500_000);
        let required: u64 = 1_000_000;
        let delta = post.unwrap_or(0).saturating_sub(pre.unwrap_or(0));
        assert_eq!(delta, required);
    }

    #[test]
    fn test_balance_delta_mismatch_rejected() {
        let pre: Option<u64> = Some(0);
        let post: Option<u64> = Some(999_999);
        let required: u64 = 1_000_000;
        let delta = post.unwrap_or(0).saturating_sub(pre.unwrap_or(0));
        assert_ne!(delta, required, "underpayment must fail delta check");
    }

    #[test]
    fn test_balance_delta_saturation_prevents_overflow() {
        // Attacker tries to trick delta by making post < pre.
        let pre: Option<u64> = Some(2_000_000);
        let post: Option<u64> = Some(1_000_000);
        let required: u64 = 1_000_000;
        // saturating_sub returns 0, not negative, so this would not pass the check.
        let delta = post.unwrap_or(0).saturating_sub(pre.unwrap_or(0));
        assert_eq!(delta, 0);
        assert_ne!(delta, required);
    }
}

/// A submission whose outcome is unknown keeps its signature.
///
/// `sendTransaction` answered by a gateway error and a status read that fails
/// after the RPC accepted the transaction both leave it able to land, so the
/// error is `SettlementUnconfirmed` under the transaction's own signature. A
/// node that refused it keeps the old error: nothing left.
#[cfg(test)]
mod submission_outcome_tests {
    use super::*;
    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use serde_json::{json, Value};
    use solana_client::client_error::{ClientError, ClientErrorKind};
    use solana_client::rpc_request::{RpcError, RpcResponseErrorData};
    use solana_sdk::hash::Hash;
    use solana_sdk::instruction::Instruction;
    use solana_sdk::transaction::Transaction;

    #[test]
    fn only_a_refusal_proves_a_submission_never_left() {
        let never_left: Vec<ClientError> = vec![
            ClientErrorKind::RpcError(RpcError::RpcResponseError {
                code: -32002,
                message: "Transaction simulation failed: Blockhash not found".into(),
                data: RpcResponseErrorData::Empty,
            })
            .into(),
            ClientErrorKind::Custom("Serialization failed: io error".into()).into(),
        ];
        for error in never_left {
            assert!(!send_may_have_landed(&error), "{error}");
        }
        let may_have_left: Vec<ClientError> = vec![
            ClientErrorKind::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out",
            ))
            .into(),
            ClientErrorKind::RpcError(RpcError::RpcRequestError(
                "RPC node returned mismatched signature".into(),
            ))
            .into(),
            ClientErrorKind::RpcError(RpcError::ParseError("Signature".into())).into(),
            ClientErrorKind::SerdeJson(serde_json::from_str::<u8>("x").unwrap_err()).into(),
        ];
        for error in may_have_left {
            assert!(send_may_have_landed(&error), "{error}");
        }
    }

    /// A preflight saying the transaction was already processed is not a
    /// refusal: it may have landed. Read from the simulation's own error and,
    /// with no simulation data, from the text.
    #[test]
    fn an_already_processed_preflight_may_have_landed() {
        let simulation: solana_client::rpc_response::RpcSimulateTransactionResult =
            serde_json::from_value(json!({"err": "AlreadyProcessed", "logs": []})).unwrap();
        let with_data: ClientError = ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32002,
            message: "Transaction simulation failed".into(),
            data: RpcResponseErrorData::SendTransactionPreflightFailure(simulation),
        })
        .into();
        assert!(send_may_have_landed(&with_data));
        let text_only: ClientError = ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32002,
            message: "Transaction simulation failed: This transaction has already been processed"
                .into(),
            data: RpcResponseErrorData::Empty,
        })
        .into();
        assert!(send_may_have_landed(&text_only));
    }

    #[derive(Clone, Copy)]
    enum Node {
        RefusesTheSend,
        LosesTheSendAnswer,
        LosesTheStatus,
    }

    async fn rpc(
        State((node, sig)): State<(Node, String)>,
        Json(req): Json<Value>,
    ) -> axum::response::Response {
        let id = req["id"].clone();
        let answer = |result: Value| Json(json!({"jsonrpc":"2.0","id":id,"result":result}));
        match (req["method"].as_str().unwrap_or_default(), node) {
            ("sendTransaction", Node::RefusesTheSend) => Json(json!({"jsonrpc":"2.0","id":id,
                "error":{"code":-32002,"message":"Transaction simulation failed: Blockhash not found"}}))
            .into_response(),
            ("sendTransaction", Node::LosesTheSendAnswer) => {
                (axum::http::StatusCode::BAD_GATEWAY, "upstream reset").into_response()
            }
            ("sendTransaction", Node::LosesTheStatus) => answer(json!(sig)).into_response(),
            ("getSignatureStatuses", _) => {
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "status backend down").into_response()
            }
            _ => answer(Value::Null).into_response(),
        }
    }

    async fn submit_against(node: Node) -> (Result<Signature, FacilitatorLocalError>, Signature) {
        let payer = Keypair::new();
        let instruction = Instruction::new_with_bytes(Pubkey::new_unique(), &[], vec![]);
        let tx = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::default(),
        );
        let tx = TransactionInt::new(VersionedTransaction::from(tx));
        let sig = tx.inner.signatures[0];

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/", post(rpc))
            .with_state((node, sig.to_string()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = RpcClient::new(format!("http://{addr}/"));
        let result = tx
            .send_and_confirm(&client, CommitmentConfig::confirmed(), Network::Solana)
            .await;
        (result, sig)
    }

    #[tokio::test]
    async fn a_submission_whose_answer_is_lost_reports_its_signature() {
        for node in [Node::LosesTheSendAnswer, Node::LosesTheStatus] {
            let (result, sig) = submit_against(node).await;
            match result {
                Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                    assert_eq!(tx, TransactionHash::Solana(*sig.as_array()));
                    assert_eq!(network, Network::Solana);
                }
                other => panic!("expected SettlementUnconfirmed under {sig}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_submission_the_node_refused_keeps_its_old_error() {
        let (result, _) = submit_against(Node::RefusesTheSend).await;
        match result {
            Err(FacilitatorLocalError::ContractCall(message)) => {
                assert!(message.contains("Blockhash not found"), "{message}");
            }
            other => panic!("expected the refusal as ContractCall, got {other:?}"),
        }
    }
}

/// The settlement-account sweep is a send after which the deposit is already
/// marked used: a sweep whose answer is lost, or that the node says it already
/// processed, is unconfirmed under the sweep's own signature; a refusal keeps
/// its error.
#[cfg(test)]
mod sweep_outcome_tests {
    use super::*;
    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use base64::Engine as _;
    use serde_json::{json, Value};
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    #[derive(Clone, Copy)]
    enum Node {
        LosesTheSendAnswer,
        AlreadyProcessed,
        Refuses,
        /// Takes the sweep and reports it confirmed.
        Confirms,
    }

    #[derive(Clone)]
    struct Stub {
        node: Node,
        sent: StdArc<StdMutex<Vec<String>>>,
        /// What `getTokenAccountBalance` answers for the settlement account.
        balance: u64,
        /// What `getTransaction` answers for the cited deposit.
        cited: Value,
        /// Whether `getAccountInfo` finds payTo's token account.
        pay_to_ata_exists: bool,
    }

    fn stub(node: Node, balance: u64) -> Stub {
        Stub {
            node,
            sent: Default::default(),
            balance,
            cited: Value::Null,
            pay_to_ata_exists: true,
        }
    }

    async fn rpc(State(stub): State<Stub>, Json(req): Json<Value>) -> axum::response::Response {
        let id = req["id"].clone();
        let answer = |result: Value| Json(json!({"jsonrpc":"2.0","id":id,"result":result}));
        let refuse = |message: &str, data: Value| {
            Json(json!({"jsonrpc":"2.0","id":req["id"],
                "error":{"code":-32002,"message":message,"data":data}}))
            .into_response()
        };
        match req["method"].as_str().unwrap_or_default() {
            "getTokenAccountBalance" => answer(json!({"context":{"slot":1},
                "value":{"amount":stub.balance.to_string(),"decimals":6,"uiAmount":null,"uiAmountString":"0"}}))
            .into_response(),
            "getTransaction" => answer(stub.cited.clone()).into_response(),
            "getAccountInfo" => {
                let token_account = json!({"lamports":2_039_280u64,
                    "data":[base64::engine::general_purpose::STANDARD.encode([0u8; 165]),"base64"],
                    "owner":spl_token::id().to_string(),"executable":false,"rentEpoch":0,"space":165});
                let value = if stub.pay_to_ata_exists {
                    token_account
                } else {
                    Value::Null
                };
                answer(json!({"context":{"slot":1},"value":value})).into_response()
            }
            "getLatestBlockhash" => answer(json!({"context":{"slot":1},
                "value":{"blockhash":solana_sdk::hash::Hash::default().to_string(),"lastValidBlockHeight":100}}))
            .into_response(),
            "getSignatureStatuses" => answer(json!({"context":{"slot":1},"value":[{
                "slot":1,"confirmations":null,"status":{"Ok":null},"err":null,
                "confirmationStatus":"confirmed"}]}))
            .into_response(),
            "sendTransaction" => {
                let raw = req["params"][0].as_str().unwrap_or_default().to_string();
                stub.sent.lock().unwrap().push(raw.clone());
                match stub.node {
                    Node::LosesTheSendAnswer => {
                        (axum::http::StatusCode::BAD_GATEWAY, "upstream reset").into_response()
                    }
                    Node::AlreadyProcessed => refuse(
                        "Transaction simulation failed: This transaction has already been processed",
                        json!({"err":"AlreadyProcessed","logs":[]}),
                    ),
                    Node::Refuses => refuse(
                        "Transaction simulation failed: Blockhash not found",
                        json!({"err":"BlockhashNotFound","logs":[]}),
                    ),
                    Node::Confirms => answer(json!(signature_of(&raw).to_string())).into_response(),
                }
            }
            _ => answer(Value::Null).into_response(),
        }
    }

    /// Starts the stub node and returns a provider whose RPC it is.
    async fn provider_for(stub: Stub) -> SolanaProvider {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/", post(rpc)).with_state(stub);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        SolanaProvider {
            keypair: Arc::new(Keypair::new()),
            chain: SolanaChain::try_from(Network::Solana).unwrap(),
            rpc_client: Arc::new(RpcClient::new(format!("http://{addr}/"))),
            max_compute_unit_limit: 400_000,
            max_compute_unit_price: 1_000_000,
        }
    }

    fn requirements(pay_to: &Pubkey, asset: &Pubkey, amount: u64) -> PaymentRequirements {
        serde_json::from_value(json!({
            "scheme":"exact","network":"solana","maxAmountRequired":amount.to_string(),
            "resource":"https://merchant.example/data","description":"sweep fixture",
            "mimeType":"application/json","payTo":pay_to.to_string(),
            "maxTimeoutSeconds":60,"asset":asset.to_string()
        }))
        .unwrap()
    }

    /// A payload citing a fresh deposit signature.
    fn payload_for(
        settlement: Option<&Keypair>,
        rent_destination: Option<&Pubkey>,
    ) -> crate::types::SettlementAccountPayload {
        crate::types::SettlementAccountPayload {
            transaction_signature: Keypair::new().sign_message(b"deposit").to_string(),
            settle_secret_key: settlement.map(|k| k.to_base58_string()),
            settlement_rent_destination: rent_destination.map(|p| p.to_string()),
        }
    }

    /// `getTransaction` for a confirmed deposit in which `buyer` sent `amount`
    /// of `mint` to the token account owned by `credited`.
    fn deposit(mint: &Pubkey, buyer: &Pubkey, credited: &Pubkey, amount: u64) -> Value {
        let balance = |index: u8, owner: &Pubkey, amount: u64| {
            json!({
                "accountIndex": index,
                "mint": mint.to_string(),
                "uiTokenAmount": {"uiAmount": null, "decimals": 6,
                    "amount": amount.to_string(), "uiAmountString": "0"},
                "owner": owner.to_string(),
                "programId": spl_token::id().to_string(),
            })
        };
        json!({
            "slot": 1,
            "blockTime": null,
            "transaction": ["", "base64"],
            "meta": {
                "err": null, "status": {"Ok": null}, "fee": 5000,
                "preBalances": [], "postBalances": [],
                "preTokenBalances": [balance(1, buyer, amount)],
                "postTokenBalances": [balance(1, buyer, 0), balance(2, credited, amount)],
            },
        })
    }

    async fn sweep(
        provider: &SolanaProvider,
        payload: &crate::types::SettlementAccountPayload,
        requirements: &PaymentRequirements,
    ) -> Result<SettleResponse, FacilitatorLocalError> {
        let verification = SettlementAccountVerifyResult {
            payer: SolanaAddress::from(Pubkey::new_unique()),
            tx_signature: Signature::default(),
        };
        let secret = payload.settle_secret_key.clone().unwrap();
        provider
            .sweep_settlement_account(&secret, payload, requirements, &verification)
            .await
    }

    async fn sweep_against(
        node: Node,
    ) -> (Result<SettleResponse, FacilitatorLocalError>, Vec<String>) {
        let stub = stub(node, 1000);
        let sent = stub.sent.clone();
        let provider = provider_for(stub).await;
        let settlement = Keypair::new();
        let payload = payload_for(Some(&settlement), None);
        let requirements = requirements(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);
        let result = sweep(&provider, &payload, &requirements).await;
        let sent = sent.lock().unwrap().clone();
        (result, sent)
    }

    /// The signature of the sweep the node was handed.
    fn signature_of(raw: &str) -> Signature {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .unwrap();
        let tx: VersionedTransaction = bincode::deserialize(&bytes).unwrap();
        tx.signatures[0]
    }

    #[tokio::test]
    async fn a_sweep_that_may_have_landed_reports_its_signature() {
        for node in [Node::LosesTheSendAnswer, Node::AlreadyProcessed] {
            let (result, sent) = sweep_against(node).await;
            assert_eq!(sent.len(), 1, "the sweep was sent again");
            match result {
                Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                    assert_eq!(
                        tx,
                        TransactionHash::Solana(*signature_of(&sent[0]).as_array())
                    );
                    assert_eq!(network, Network::Solana);
                }
                other => panic!("expected SettlementUnconfirmed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_sweep_the_node_refused_keeps_its_error() {
        let (result, _) = sweep_against(Node::Refuses).await;
        match result {
            Err(FacilitatorLocalError::ContractCall(message)) => {
                assert!(
                    message.starts_with("settlement account sweep failed"),
                    "{message}"
                );
                assert!(message.contains("Blockhash not found"), "{message}");
            }
            other => panic!("expected the refusal, got {other:?}"),
        }
    }

    // ---- What a settlement account must hold before it is paid out --------

    /// A sweep moves the price or nothing: a settlement account holding less
    /// is refused before any transaction is built.
    #[tokio::test]
    async fn a_settlement_account_below_the_price_is_refused_before_anything_is_sent() {
        let stub = stub(Node::Confirms, 1_000);
        let sent = stub.sent.clone();
        let provider = provider_for(stub).await;
        let settlement = Keypair::new();
        let payload = payload_for(Some(&settlement), None);
        let requirements = requirements(&Pubkey::new_unique(), &Pubkey::new_unique(), 1_000_000);

        let result = sweep(&provider, &payload, &requirements).await;
        let sent = sent.lock().unwrap().clone();

        assert!(sent.is_empty(), "a sweep below the price was sent");
        match result {
            Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Solana(account))) => {
                assert_eq!(account, settlement.pubkey());
            }
            other => panic!("expected insufficient funds, got {other:?}"),
        }
    }

    /// The cited deposit counts only if it landed in the account the payload's
    /// settleSecretKey controls, which is the account the sweep pays out of.
    #[tokio::test]
    async fn a_deposit_to_another_account_does_not_verify_a_settlement_account() {
        let (mint, buyer, elsewhere) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let mut stub = stub(Node::Refuses, 1_000);
        stub.cited = deposit(&mint, &buyer, &elsewhere, 1_000);
        let provider = provider_for(stub).await;
        let settlement = Keypair::new();
        let payload = payload_for(Some(&settlement), None);

        let result = provider
            .verify_settlement_account(&payload, &requirements(&Pubkey::new_unique(), &mint, 1_000))
            .await;

        match result {
            Err(FacilitatorLocalError::DecodingError(reason)) => {
                assert!(reason.contains("settlement account"), "{reason}");
            }
            Ok(_) => panic!("verified a deposit that did not reach the settlement account"),
            Err(other) => panic!("expected a decoding verdict, got {other:?}"),
        }
    }

    /// A deposit that did reach the settlement account is not enough if the
    /// account no longer holds the price: verify reads what the sweep would move.
    #[tokio::test]
    async fn a_settlement_account_holding_less_than_the_price_does_not_verify() {
        let (mint, buyer) = (Pubkey::new_unique(), Pubkey::new_unique());
        let settlement = Keypair::new();
        let mut stub = stub(Node::Refuses, 1);
        stub.cited = deposit(&mint, &buyer, &settlement.pubkey(), 1_000);
        let provider = provider_for(stub).await;
        let payload = payload_for(Some(&settlement), None);

        let result = provider
            .verify_settlement_account(&payload, &requirements(&Pubkey::new_unique(), &mint, 1_000))
            .await;

        match result {
            Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Solana(account))) => {
                assert_eq!(account, settlement.pubkey());
            }
            Ok(_) => panic!("verified a settlement account that holds less than the price"),
            Err(other) => panic!("expected insufficient funds, got {other:?}"),
        }
    }

    /// The flow a custodial client runs today: a deposit of the price into a
    /// fresh settlement account it then hands over. Still verifies, with the
    /// depositor as payer.
    #[tokio::test]
    async fn a_deposit_of_the_price_into_the_settlement_account_still_verifies() {
        let (mint, buyer) = (Pubkey::new_unique(), Pubkey::new_unique());
        let settlement = Keypair::new();
        let mut stub = stub(Node::Refuses, 1_000);
        stub.cited = deposit(&mint, &buyer, &settlement.pubkey(), 1_000);
        let provider = provider_for(stub).await;
        let payload = payload_for(Some(&settlement), None);

        let verified = provider
            .verify_settlement_account(&payload, &requirements(&Pubkey::new_unique(), &mint, 1_000))
            .await
            .expect("a deposit of the price into the settlement account verifies");

        assert_eq!(verified.payer.pubkey, buyer);
    }

    /// Without a settleSecretKey nothing is swept, so the cited transaction must
    /// have paid payTo itself: unchanged.
    #[tokio::test]
    async fn a_direct_payment_to_pay_to_still_verifies_without_a_settle_key() {
        let (mint, buyer, pay_to) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let mut stub = stub(Node::Refuses, 0);
        stub.cited = deposit(&mint, &buyer, &pay_to, 1_000);
        let provider = provider_for(stub).await;
        let payload = payload_for(None, None);

        let verified = provider
            .verify_settlement_account(&payload, &requirements(&pay_to, &mint, 1_000))
            .await
            .expect("a direct payment to payTo verifies");

        assert_eq!(verified.payer.pubkey, buyer);
    }

    fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[owner.as_ref(), spl_token::id().as_ref(), mint.as_ref()],
            &ATA_PROGRAM_PUBKEY,
        )
        .0
    }

    /// Program, accounts and data of one instruction of a sent transaction.
    type Ix = (Pubkey, Vec<Pubkey>, Vec<u8>);

    /// A TransferChecked of `amount` from `source` to `destination`.
    fn is_transfer(ix: &Ix, source: &Pubkey, destination: &Pubkey, amount: u64) -> bool {
        let (program, accounts, data) = ix;
        *program == spl_token::id()
            && data.first() == Some(&12)
            && data.get(1..9) == Some(&amount.to_le_bytes()[..])
            && accounts.first() == Some(source)
            && accounts.get(2) == Some(destination)
    }

    /// A CloseAccount of `account` that sends its rent to `destination`.
    fn is_close(ix: &Ix, account: &Pubkey, destination: &Pubkey) -> bool {
        let (program, accounts, data) = ix;
        *program == spl_token::id()
            && data.first() == Some(&9)
            && accounts.first() == Some(account)
            && accounts.get(1) == Some(destination)
    }

    fn instructions_of(raw: &str) -> Vec<Ix> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .unwrap();
        let tx: VersionedTransaction = bincode::deserialize(&bytes).unwrap();
        let keys = tx.message.static_account_keys();
        tx.message
            .instructions()
            .iter()
            .map(|ix| {
                (
                    keys[ix.program_id_index as usize],
                    ix.accounts.iter().map(|i| keys[*i as usize]).collect(),
                    ix.data.clone(),
                )
            })
            .collect()
    }

    /// The custodial flow as a client runs it today, through settle: a deposit
    /// of the price into a fresh settlement account verifies, the sweep moves
    /// the price to payTo and closes the settlement account to the rent
    /// destination the client named, and the answer names the sweep.
    #[tokio::test]
    async fn a_custodial_payment_still_settles_end_to_end() {
        let (mint, buyer, pay_to, rent_back) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let settlement = Keypair::new();
        let mut stub = stub(Node::Confirms, 1_000);
        stub.cited = deposit(&mint, &buyer, &settlement.pubkey(), 1_000);
        let sent = stub.sent.clone();
        let provider = provider_for(stub).await;
        let payload = payload_for(Some(&settlement), Some(&rent_back));

        let settled = provider
            .settle_settlement_account(&payload, &requirements(&pay_to, &mint, 1_000))
            .await
            .expect("a custodial payment settles");

        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "exactly one sweep");
        assert!(settled.success);
        assert_eq!(settled.payer, MixedAddress::Solana(buyer));
        assert_eq!(
            settled.transaction,
            Some(TransactionHash::Solana(*signature_of(&sent[0]).as_array()))
        );
        let settlement_ata = ata(&settlement.pubkey(), &mint);
        let pay_to_ata = ata(&pay_to, &mint);
        let instructions = instructions_of(&sent[0]);
        assert!(
            instructions
                .iter()
                .any(|ix| is_transfer(ix, &settlement_ata, &pay_to_ata, 1_000)),
            "the sweep moves the price from the settlement account to payTo"
        );
        assert!(
            instructions
                .iter()
                .any(|ix| is_close(ix, &settlement_ata, &rent_back)),
            "the settlement account is closed to the client's rent destination"
        );
    }

    // ---- Who pays for payTo's token account ---------------------------------

    /// The fee payer funds an associated token account only in a transaction
    /// that also closes the settlement account with its rent going to the fee
    /// payer, which pays that account back.
    fn assert_rent_comes_back(raw: &str, fee_payer: &Pubkey, settlement_ata: &Pubkey) {
        let instructions = instructions_of(raw);
        let funds = instructions.iter().any(|(program, accounts, _)| {
            *program == ATA_PROGRAM_PUBKEY && accounts.first() == Some(fee_payer)
        });
        let paid_back = instructions
            .iter()
            .any(|ix| is_close(ix, settlement_ata, fee_payer));
        assert!(
            !funds || paid_back,
            "the fee payer funds an account whose rent does not come back to it"
        );
    }

    fn creates_an_account(raw: &str) -> bool {
        instructions_of(raw)
            .iter()
            .any(|(program, _, _)| *program == ATA_PROGRAM_PUBKEY)
    }

    struct Swept {
        result: Result<SettleResponse, FacilitatorLocalError>,
        sent: Vec<String>,
        fee_payer: Pubkey,
        settlement_ata: Pubkey,
        pay_to_ata: Pubkey,
    }

    /// A sweep of a settlement account holding `balance` for a price of
    /// `required`, to a payTo whose token account exists or not.
    async fn sweep_to(
        balance: u64,
        required: u64,
        pay_to_ata_exists: bool,
        rent_destination: Option<&Pubkey>,
    ) -> Swept {
        let mut stub = stub(Node::Confirms, balance);
        stub.pay_to_ata_exists = pay_to_ata_exists;
        let sent = stub.sent.clone();
        let provider = provider_for(stub).await;
        let (mint, pay_to, settlement) =
            (Pubkey::new_unique(), Pubkey::new_unique(), Keypair::new());
        let payload = payload_for(Some(&settlement), rent_destination);
        let result = sweep(&provider, &payload, &requirements(&pay_to, &mint, required)).await;
        let sent = sent.lock().unwrap().clone();
        Swept {
            result,
            sent,
            fee_payer: provider.keypair.pubkey(),
            settlement_ata: ata(&settlement.pubkey(), &mint),
            pay_to_ata: ata(&pay_to, &mint),
        }
    }

    /// A sweep that leaves the settlement account open has no rent to pay the
    /// fee payer back with, so it never creates payTo's account: it is refused.
    #[tokio::test]
    async fn an_overfunded_sweep_never_creates_a_missing_pay_to_account() {
        let swept = sweep_to(2_000, 1_000, false, None).await;

        for raw in &swept.sent {
            assert_rent_comes_back(raw, &swept.fee_payer, &swept.settlement_ata);
        }
        assert!(
            swept.sent.is_empty(),
            "a sweep to a missing account was sent"
        );
        assert!(
            matches!(swept.result, Err(FacilitatorLocalError::DecodingError(_))),
            "{:?}",
            swept.result
        );
    }

    /// A full sweep may create payTo's account, and then the settlement
    /// account's rent pays the fee payer back, whatever the client asked for.
    #[tokio::test]
    async fn a_sweep_that_creates_the_pay_to_account_sends_the_rent_to_the_fee_payer() {
        let client = Pubkey::new_unique();
        let swept = sweep_to(1_000, 1_000, false, Some(&client)).await;

        assert_eq!(swept.sent.len(), 1, "exactly one sweep");
        assert_rent_comes_back(&swept.sent[0], &swept.fee_payer, &swept.settlement_ata);
        let instructions = instructions_of(&swept.sent[0]);
        assert!(
            instructions.iter().any(|(program, accounts, _)| {
                *program == ATA_PROGRAM_PUBKEY && accounts.get(1) == Some(&swept.pay_to_ata)
            }),
            "payTo's account is created"
        );
        assert!(
            instructions.iter().any(|ix| is_transfer(
                ix,
                &swept.settlement_ata,
                &swept.pay_to_ata,
                1_000
            )),
            "the price moves to payTo"
        );
        assert!(swept.result.is_ok(), "{:?}", swept.result);
    }

    /// An existing payTo account gets no create instruction, so nothing in the
    /// sweep can make the fee payer fund one.
    #[tokio::test]
    async fn an_overfunded_sweep_to_an_existing_account_creates_nothing() {
        let swept = sweep_to(2_000, 1_000, true, None).await;

        assert_eq!(swept.sent.len(), 1, "exactly one sweep");
        assert_rent_comes_back(&swept.sent[0], &swept.fee_payer, &swept.settlement_ata);
        assert!(
            !creates_an_account(&swept.sent[0]),
            "an account was created"
        );
        let instructions = instructions_of(&swept.sent[0]);
        assert!(
            instructions.iter().any(|ix| is_transfer(
                ix,
                &swept.settlement_ata,
                &swept.pay_to_ata,
                1_000
            )),
            "the price moves to payTo"
        );
        assert!(swept.result.is_ok(), "{:?}", swept.result);
    }

    /// A full sweep to an existing account creates nothing and closes the
    /// settlement account to the rent destination the client named.
    #[tokio::test]
    async fn a_full_sweep_to_an_existing_account_returns_the_rent_to_the_client() {
        let client = Pubkey::new_unique();
        let swept = sweep_to(1_000, 1_000, true, Some(&client)).await;

        assert_eq!(swept.sent.len(), 1, "exactly one sweep");
        assert_rent_comes_back(&swept.sent[0], &swept.fee_payer, &swept.settlement_ata);
        assert!(
            !creates_an_account(&swept.sent[0]),
            "an account was created"
        );
        assert!(
            instructions_of(&swept.sent[0]).iter().any(|ix| is_close(
                ix,
                &swept.settlement_ata,
                &client
            )),
            "the rent goes to the client's destination"
        );
        assert!(swept.result.is_ok(), "{:?}", swept.result);
    }

    // ---- When the deposit's signature is claimed -----------------------------

    fn is_replay(result: &Result<SettleResponse, FacilitatorLocalError>) -> bool {
        matches!(result, Err(FacilitatorLocalError::DecodingError(reason))
            if reason.contains("replay protection"))
    }

    /// A settle that fails verification claims nothing: the same deposit then
    /// settles for the payment it belongs to, and only once.
    #[tokio::test]
    async fn a_settle_that_fails_verification_leaves_the_deposit_unclaimed() {
        let (mint, buyer, pay_to) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let settlement = Keypair::new();
        let mut stub = stub(Node::Confirms, 1_000);
        stub.cited = deposit(&mint, &buyer, &settlement.pubkey(), 1_000);
        let provider = provider_for(stub).await;
        let payload = payload_for(Some(&settlement), None);

        let refused = provider
            .settle_settlement_account(&payload, &requirements(&pay_to, &mint, 2_000))
            .await;
        assert!(refused.is_err() && !is_replay(&refused), "{refused:?}");

        let settled = provider
            .settle_settlement_account(&payload, &requirements(&pay_to, &mint, 1_000))
            .await;
        assert!(settled.as_ref().is_ok_and(|s| s.success), "{settled:?}");

        let again = provider
            .settle_settlement_account(&payload, &requirements(&pay_to, &mint, 1_000))
            .await;
        assert!(is_replay(&again), "{again:?}");
    }

    /// A sweep the node refused moved nothing, so it gives the claim back and
    /// the same deposit can be settled again.
    #[tokio::test]
    async fn a_sweep_that_moved_nothing_gives_the_deposit_back() {
        let (mint, buyer, pay_to) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let settlement = Keypair::new();
        let node = |kind| {
            let mut stub = stub(kind, 1_000);
            stub.cited = deposit(&mint, &buyer, &settlement.pubkey(), 1_000);
            stub
        };
        let payload = payload_for(Some(&settlement), None);
        let requirements = requirements(&pay_to, &mint, 1_000);

        let refused = provider_for(node(Node::Refuses))
            .await
            .settle_settlement_account(&payload, &requirements)
            .await;
        assert!(
            matches!(refused, Err(FacilitatorLocalError::ContractCall(_))),
            "{refused:?}"
        );

        let settled = provider_for(node(Node::Confirms))
            .await
            .settle_settlement_account(&payload, &requirements)
            .await;
        assert!(settled.as_ref().is_ok_and(|s| s.success), "{settled:?}");
    }

    /// A sweep with no verdict may still land, so the deposit stays claimed.
    #[tokio::test]
    async fn a_sweep_that_may_have_landed_keeps_the_deposit_claimed() {
        let (mint, buyer, pay_to) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let settlement = Keypair::new();
        let node = |kind| {
            let mut stub = stub(kind, 1_000);
            stub.cited = deposit(&mint, &buyer, &settlement.pubkey(), 1_000);
            stub
        };
        let payload = payload_for(Some(&settlement), None);
        let requirements = requirements(&pay_to, &mint, 1_000);

        let unconfirmed = provider_for(node(Node::LosesTheSendAnswer))
            .await
            .settle_settlement_account(&payload, &requirements)
            .await;
        assert!(
            matches!(
                unconfirmed,
                Err(FacilitatorLocalError::SettlementUnconfirmed(..))
            ),
            "{unconfirmed:?}"
        );

        let again = provider_for(node(Node::Confirms))
            .await
            .settle_settlement_account(&payload, &requirements)
            .await;
        assert!(is_replay(&again), "{again:?}");
    }

    // ---- Which keypair a settleSecretKey decodes to ---------------------------

    /// base58 of raw bytes, the way a client encodes the 64-byte keypair
    /// (secret half, then public half) it sends as settleSecretKey.
    fn base58(bytes: &[u8]) -> String {
        solana_sdk::bs58::encode(bytes).into_string()
    }

    /// The public half of the settlement keypair is derived from the secret
    /// half, never read from the bytes: 64 bytes whose public half is another
    /// key's are refused, though both halves are well formed.
    #[test]
    fn a_settle_secret_key_whose_public_half_is_not_derived_from_its_secret_half_is_refused() {
        let settlement = Keypair::new();
        let foreign = Keypair::new();
        let mut bytes = settlement.to_bytes();
        bytes[32..].copy_from_slice(&foreign.pubkey().to_bytes());

        match settlement_keypair(&base58(&bytes)) {
            Err(FacilitatorLocalError::DecodingError(reason)) => {
                assert!(reason.contains("settleSecretKey"), "{reason}");
            }
            Ok(keypair) => panic!(
                "accepted a public half the secret half does not derive (decoded {})",
                keypair.pubkey()
            ),
            Err(other) => panic!("expected a decoding error, got {other:?}"),
        }
    }

    /// A well-formed settleSecretKey, encoded as the payload carries it or as
    /// plain base58 of its 64 bytes, decodes to the keypair it encodes.
    #[test]
    fn a_well_formed_settle_secret_key_decodes_to_its_keypair() {
        let settlement = Keypair::new();
        for wire in [
            settlement.to_base58_string(),
            base58(&settlement.to_bytes()),
        ] {
            let keypair = match settlement_keypair(&wire) {
                Ok(keypair) => keypair,
                Err(error) => panic!("refused a well-formed settleSecretKey: {error:?}"),
            };
            assert_eq!(keypair.pubkey(), settlement.pubkey());
            assert!(
                keypair.secret_bytes() == settlement.secret_bytes(),
                "the secret half changed"
            );
        }
    }

    /// Anything but base58 of exactly 64 bytes is refused: text outside the
    /// base58 alphabet, nothing, the secret half alone, a byte short and a
    /// byte over.
    #[test]
    fn a_settle_secret_key_that_is_not_64_bytes_of_base58_is_refused() {
        let bytes = Keypair::new().to_bytes();
        let mut one_over = bytes.to_vec();
        one_over.push(0);
        let cases = [
            ("outside the base58 alphabet", "0OIl+/".to_string()),
            ("empty", String::new()),
            ("the secret half alone", base58(&bytes[..32])),
            ("63 bytes", base58(&bytes[..63])),
            ("65 bytes", base58(&one_over)),
        ];
        for (case, wire) in cases {
            match settlement_keypair(&wire) {
                Err(FacilitatorLocalError::DecodingError(_)) => {}
                Ok(keypair) => panic!("{case}: decoded a keypair for {}", keypair.pubkey()),
                Err(other) => panic!("{case}: expected a decoding error, got {other:?}"),
            }
        }
    }
}

/// A payment made through a program the payer's wallet runs (a smart wallet)
/// is verified on a simulation of its transaction; the settle answer rests on
/// the transaction that landed, read back from the chain once it confirmed. A
/// standard payment's transfer is a top-level instruction of its transaction,
/// so its confirmation is the answer, as before.
#[cfg(test)]
mod landed_payment_tests {
    use super::*;
    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use base64::Engine as _;
    use serde_json::{json, Value};
    use solana_sdk::hash::Hash;
    use solana_sdk::instruction::{AccountMeta, Instruction};
    use solana_sdk::message::Message;
    use solana_sdk::transaction::Transaction;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    /// The price of every payment here, in USDC base units.
    const PRICE: u64 = 1_000;

    /// USDC on Solana, on the network's asset allow-list.
    fn usdc() -> Pubkey {
        Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
    }

    fn b64(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[owner.as_ref(), spl_token::ID.as_ref(), mint.as_ref()],
            &ATA_PROGRAM_PUBKEY,
        )
        .0
    }

    /// An SPL token account of `mint`, owned by `owner`, holding `amount`.
    fn token_account(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Value {
        let mut data = [0u8; 165];
        data[..32].copy_from_slice(mint.as_ref());
        data[32..64].copy_from_slice(owner.as_ref());
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        data[108] = 1; // initialized
        json!({"lamports": 2_039_280u64, "data": [b64(&data), "base64"],
            "owner": spl_token::ID.to_string(), "executable": false, "rentEpoch": 0,
            "space": 165})
    }

    /// The signature of the transaction the node was handed: its id.
    fn signature_of(raw: &str) -> Signature {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .unwrap();
        let tx: VersionedTransaction = bincode::deserialize(&bytes).unwrap();
        tx.signatures[0]
    }

    /// The node the provider talks to.
    #[derive(Clone)]
    struct Chain {
        mint: Pubkey,
        pay_to: Pubkey,
        /// payTo's token account before the payment; None: it does not exist.
        before: Option<u64>,
        /// payTo's token account as the simulation leaves it; None: the
        /// simulation does not return it.
        simulated: Option<u64>,
        /// The inner instructions the simulation reports.
        inner: Value,
        /// What `getTransaction` answers for the confirmed transaction; null:
        /// the node never finds it.
        landed: Value,
        sent: StdArc<StdMutex<Vec<String>>>,
        reads: StdArc<AtomicUsize>,
    }

    impl Chain {
        fn new(mint: Pubkey, pay_to: Pubkey) -> Self {
            Self {
                mint,
                pay_to,
                before: Some(0),
                simulated: Some(PRICE),
                inner: json!([]),
                landed: Value::Null,
                sent: Default::default(),
                reads: Default::default(),
            }
        }
    }

    async fn rpc(State(chain): State<Chain>, Json(req): Json<Value>) -> axum::response::Response {
        let id = req["id"].clone();
        let answer =
            |result: Value| Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response();
        let pay_to_ata = ata(&chain.pay_to, &chain.mint).to_string();
        match req["method"].as_str().unwrap_or_default() {
            "getTokenAccountBalance" => match chain.before {
                Some(amount) if req["params"][0] == json!(pay_to_ata) => answer(
                    json!({"context":{"slot":1},"value":{"amount":amount.to_string(),
                        "decimals":6,"uiAmount":null,"uiAmountString":"0"}}),
                ),
                _ => Json(json!({"jsonrpc":"2.0","id":req["id"],
                    "error":{"code":-32602,"message":"Invalid param: could not find account"}}))
                .into_response(),
            },
            "getMultipleAccounts" => {
                let accounts: Vec<Value> = req["params"][0]
                    .as_array()
                    .map(|keys| {
                        keys.iter()
                            .map(|_| token_account(&chain.mint, &chain.pay_to, 0))
                            .collect()
                    })
                    .unwrap_or_default();
                answer(json!({"context":{"slot":1},"value":accounts}))
            }
            "simulateTransaction" => {
                let pay_to_account = chain
                    .simulated
                    .map(|amount| token_account(&chain.mint, &chain.pay_to, amount))
                    .unwrap_or(Value::Null);
                answer(json!({"context":{"slot":1},"value":{"err":null,"logs":[],
                    "accounts":[pay_to_account, null],"unitsConsumed":1000,
                    "innerInstructions":chain.inner}}))
            }
            "sendTransaction" => {
                let raw = req["params"][0].as_str().unwrap_or_default().to_string();
                chain.sent.lock().unwrap().push(raw.clone());
                answer(json!(signature_of(&raw).to_string()))
            }
            "getSignatureStatuses" => answer(json!({"context":{"slot":1},"value":[{
                "slot":1,"confirmations":null,"status":{"Ok":null},"err":null,
                "confirmationStatus":"confirmed"}]})),
            "getTransaction" => {
                chain.reads.fetch_add(1, Ordering::SeqCst);
                answer(chain.landed.clone())
            }
            _ => answer(Value::Null),
        }
    }

    fn compute_budget() -> Vec<Instruction> {
        let mut limit = vec![2u8];
        limit.extend_from_slice(&200_000u32.to_le_bytes());
        let mut price = vec![3u8];
        price.extend_from_slice(&1u64.to_le_bytes());
        vec![
            Instruction {
                program_id: solana_sdk::compute_budget::ID,
                accounts: vec![],
                data: limit,
            },
            Instruction {
                program_id: solana_sdk::compute_budget::ID,
                accounts: vec![],
                data: price,
            },
        ]
    }

    fn encode(tx: Transaction) -> String {
        b64(&bincode::serialize(&VersionedTransaction::from(tx)).unwrap())
    }

    /// A smart-wallet payment -- one call into the wallet's program, which
    /// moves the price out of its vault's token account into payTo's with a
    /// CPI TransferChecked -- and that CPI the way the node reports it.
    fn wallet_payment(
        fee_payer: &Pubkey,
        member: &Keypair,
        mint: &Pubkey,
        pay_to: &Pubkey,
    ) -> (String, Value) {
        let vault = Pubkey::new_unique();
        let (source, destination) = (ata(&vault, mint), ata(pay_to, mint));
        let call = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![
                AccountMeta::new_readonly(member.pubkey(), true),
                AccountMeta::new_readonly(vault, false),
                AccountMeta::new(source, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new(destination, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data: vec![1],
        };
        let mut instructions = compute_budget();
        instructions.push(call);
        let message = Message::new(&instructions, Some(fee_payer));
        let index = |key: &Pubkey| message.account_keys.iter().position(|k| k == key).unwrap();
        let mut data = vec![12u8];
        data.extend_from_slice(&PRICE.to_le_bytes());
        data.push(6);
        let inner = json!([{"index": 2, "instructions": [{
            "programIdIndex": index(&spl_token::ID),
            "accounts": [index(&source), index(mint), index(&destination), index(&vault)],
            "data": bs58::encode(&data).into_string(),
            "stackHeight": 2,
        }]}]);
        let mut tx = Transaction::new_unsigned(message);
        tx.partial_sign(&[member], Hash::default());
        (encode(tx), inner)
    }

    /// A standard payment: a top-level TransferChecked of the price from the
    /// payer's token account into payTo's.
    fn standard_payment(
        fee_payer: &Pubkey,
        payer: &Keypair,
        mint: &Pubkey,
        pay_to: &Pubkey,
    ) -> String {
        let mut instructions = compute_budget();
        instructions.push(
            spl_token::instruction::transfer_checked(
                &spl_token::ID,
                &ata(&payer.pubkey(), mint),
                mint,
                &ata(pay_to, mint),
                &payer.pubkey(),
                &[],
                PRICE,
                6,
            )
            .unwrap(),
        );
        let mut tx = Transaction::new_unsigned(Message::new(&instructions, Some(fee_payer)));
        tx.partial_sign(&[payer], Hash::default());
        encode(tx)
    }

    fn request(pay_to: &Pubkey, transaction: String) -> SettleRequest {
        serde_json::from_value(json!({
            "x402Version": 1,
            "paymentPayload": {
                "x402Version": 1, "scheme": "exact", "network": "solana",
                "payload": { "transaction": transaction },
            },
            "paymentRequirements": {
                "scheme": "exact", "network": "solana",
                "maxAmountRequired": PRICE.to_string(),
                "resource": "https://merchant.example/data", "description": "",
                "mimeType": "application/json", "payTo": pay_to.to_string(),
                "maxTimeoutSeconds": 60, "asset": usdc().to_string(),
            },
        }))
        .unwrap()
    }

    /// A token balance of `mint` at account `index`, owned by `owner`.
    fn balance(index: u8, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Value {
        json!({
            "accountIndex": index,
            "mint": mint.to_string(),
            "uiTokenAmount": {"uiAmount": null, "decimals": 6,
                "amount": amount.to_string(), "uiAmountString": "0"},
            "owner": owner.to_string(),
            "programId": spl_token::ID.to_string(),
        })
    }

    /// `getTransaction` for a confirmed transaction: its error (null: none)
    /// and its token balances before and after.
    fn landed(err: Value, before: Vec<Value>, after: Vec<Value>) -> Value {
        let status = if err.is_null() {
            json!({"Ok": null})
        } else {
            json!({"Err": err.clone()})
        };
        json!({
            "slot": 2,
            "blockTime": null,
            "transaction": ["", "base64"],
            "meta": {
                "err": err, "status": status, "fee": 5000,
                "preBalances": [], "postBalances": [],
                "preTokenBalances": before, "postTokenBalances": after,
            },
        })
    }

    struct Settled {
        result: Result<SettleResponse, FacilitatorLocalError>,
        sent: Vec<String>,
        /// How many times settle asked for the landed transaction.
        reads: usize,
    }

    async fn settle_on(
        chain: Chain,
        fee_payer: Keypair,
        pay_to: &Pubkey,
        transaction: String,
    ) -> Settled {
        let (sent, reads) = (chain.sent.clone(), chain.reads.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/", post(rpc)).with_state(chain);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = SolanaProvider {
            keypair: Arc::new(fee_payer),
            chain: SolanaChain::try_from(Network::Solana).unwrap(),
            rpc_client: Arc::new(RpcClient::new(format!("http://{addr}/"))),
            max_compute_unit_limit: 400_000,
            max_compute_unit_price: 1_000_000,
        };
        let result = provider.settle(&request(pay_to, transaction)).await;
        let sent = sent.lock().unwrap().clone();
        Settled {
            result,
            sent,
            reads: reads.load(Ordering::SeqCst),
        }
    }

    /// A smart-wallet payment to `pay_to` whose simulation leaves payTo's
    /// account holding `simulated`, settled on a chain where its transaction
    /// landed as `landed` says.
    async fn wallet_settle(pay_to: Pubkey, simulated: Option<u64>, landed: Value) -> Settled {
        let (member, fee_payer) = (Keypair::new(), Keypair::new());
        let (transaction, inner) = wallet_payment(&fee_payer.pubkey(), &member, &usdc(), &pay_to);
        let mut chain = Chain::new(usdc(), pay_to);
        chain.simulated = simulated;
        chain.inner = inner;
        chain.landed = landed;
        settle_on(chain, fee_payer, &pay_to, transaction).await
    }

    /// The price credited to payTo by the transaction that landed: a
    /// settlement, under that transaction's signature, read from the chain.
    #[tokio::test]
    async fn a_wallet_payment_that_landed_paying_pay_to_settles() {
        let (pay_to, vault) = (Pubkey::new_unique(), Pubkey::new_unique());
        let paid = landed(
            Value::Null,
            vec![
                balance(2, &usdc(), &vault, 5_000),
                balance(3, &usdc(), &pay_to, 0),
            ],
            vec![
                balance(2, &usdc(), &vault, 5_000 - PRICE),
                balance(3, &usdc(), &pay_to, PRICE),
            ],
        );

        let settled = wallet_settle(pay_to, Some(PRICE), paid).await;

        assert_eq!(settled.sent.len(), 1, "exactly one transaction sent");
        let response = settled.result.expect("a payment that landed settles");
        assert!(response.success, "{response:?}");
        assert_eq!(
            response.transaction,
            Some(TransactionHash::Solana(
                *signature_of(&settled.sent[0]).as_array()
            ))
        );
        assert!(
            settled.reads >= 1,
            "settled without reading the transaction that landed"
        );
    }

    /// What the landed transaction did to payTo's holdings of the asset is
    /// the answer. The price, or more, credited to payTo settles. A
    /// transaction that moved nothing, a credit one unit short, the price to
    /// someone else or in another token, a credit payTo's other account gave
    /// back, and a transaction that failed on chain are a payment that did
    /// not happen: `success: false`, naming no transaction.
    #[tokio::test]
    async fn only_a_landed_credit_of_the_price_to_pay_to_settles() {
        let (pay_to, vault, stranger) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let (usdc, other_mint) = (usdc(), Pubkey::new_unique());
        let failed = json!({"InstructionError": [2, {"Custom": 1}]});
        let funded = || {
            vec![
                balance(2, &usdc, &vault, 5_000),
                balance(3, &usdc, &pay_to, 0),
            ]
        };
        let paid = |amount: u64| {
            vec![
                balance(2, &usdc, &vault, 5_000 - amount),
                balance(3, &usdc, &pay_to, amount),
            ]
        };
        // (case, on-chain error, token balances before, after, settles)
        let cases: Vec<(&str, Value, Vec<Value>, Vec<Value>, bool)> = vec![
            ("nothing moved", Value::Null, funded(), funded(), false),
            ("the price", Value::Null, funded(), paid(PRICE), true),
            (
                "more than the price",
                Value::Null,
                funded(),
                paid(PRICE + 1),
                true,
            ),
            (
                "one unit short",
                Value::Null,
                funded(),
                paid(PRICE - 1),
                false,
            ),
            (
                "payTo's account created holding the price",
                Value::Null,
                vec![balance(2, &usdc, &vault, 5_000)],
                paid(PRICE),
                true,
            ),
            (
                "the price to someone else",
                Value::Null,
                vec![
                    balance(2, &usdc, &vault, 5_000),
                    balance(3, &usdc, &pay_to, 0),
                    balance(4, &usdc, &stranger, 0),
                ],
                vec![
                    balance(2, &usdc, &vault, 5_000 - PRICE),
                    balance(3, &usdc, &pay_to, 0),
                    balance(4, &usdc, &stranger, PRICE),
                ],
                false,
            ),
            (
                "the price in another token",
                Value::Null,
                vec![
                    balance(2, &usdc, &vault, 5_000),
                    balance(3, &other_mint, &pay_to, 0),
                ],
                vec![
                    balance(2, &usdc, &vault, 5_000),
                    balance(3, &other_mint, &pay_to, PRICE),
                ],
                false,
            ),
            (
                "credited, and given back by payTo's other account",
                Value::Null,
                vec![
                    balance(3, &usdc, &pay_to, 0),
                    balance(4, &usdc, &pay_to, PRICE),
                ],
                vec![
                    balance(3, &usdc, &pay_to, PRICE),
                    balance(4, &usdc, &pay_to, 0),
                ],
                false,
            ),
            ("failed on chain", failed, funded(), funded(), false),
        ];

        for (case, err, before, after, settles) in cases {
            let settled = wallet_settle(pay_to, Some(PRICE), landed(err, before, after)).await;
            let response = match settled.result {
                Ok(response) => response,
                Err(error) => panic!("{case}: expected a verdict, got {error:?}"),
            };
            assert_eq!(response.success, settles, "{case}: {response:?}");
            if settles {
                assert!(response.transaction.is_some(), "{case}: {response:?}");
            } else {
                assert_eq!(
                    response.transaction, None,
                    "{case}: a payment that did not happen names a transaction"
                );
                assert_eq!(
                    response.error_reason.as_ref().map(ToString::to_string),
                    Some("settlement_landed_without_payment".to_string()),
                    "{case}"
                );
            }
        }
    }

    /// verify judges a smart-wallet payment on its simulation, and a node can
    /// leave payTo's account out of the simulation; settle still answers from
    /// the transaction that landed.
    #[tokio::test]
    async fn a_wallet_payment_simulated_without_pay_to_account_is_judged_on_what_landed() {
        let pay_to = Pubkey::new_unique();
        let unchanged = landed(
            Value::Null,
            vec![balance(3, &usdc(), &pay_to, 0)],
            vec![balance(3, &usdc(), &pay_to, 0)],
        );

        let settled = wallet_settle(pay_to, None, unchanged).await;

        let response = settled.result.expect("a verdict on what landed");
        assert!(!response.success, "{response:?}");
        assert_eq!(response.transaction, None, "{response:?}");
    }

    /// A landed transaction the node never hands back, or whose credits it
    /// cannot attribute to an owner, or without token balances, is no
    /// verdict: unconfirmed under the transaction's signature, never a
    /// success and never a refusal a caller would answer with a new payment.
    #[tokio::test]
    async fn a_wallet_payment_whose_landed_effect_is_unknown_is_unconfirmed() {
        let (pay_to, vault) = (Pubkey::new_unique(), Pubkey::new_unique());
        let paid = landed(
            Value::Null,
            vec![
                balance(2, &usdc(), &vault, 5_000),
                balance(3, &usdc(), &pay_to, 0),
            ],
            vec![
                balance(2, &usdc(), &vault, 5_000 - PRICE),
                balance(3, &usdc(), &pay_to, PRICE),
            ],
        );
        let mut no_owners = paid.clone();
        for list in ["preTokenBalances", "postTokenBalances"] {
            for entry in no_owners["meta"][list].as_array_mut().unwrap() {
                entry.as_object_mut().unwrap().remove("owner");
            }
        }
        let mut no_balances = paid;
        for list in ["preTokenBalances", "postTokenBalances"] {
            no_balances["meta"].as_object_mut().unwrap().remove(list);
        }

        for (case, landed) in [
            ("never found", Value::Null),
            ("balances without owners", no_owners),
            ("no token balances", no_balances),
        ] {
            let settled = wallet_settle(pay_to, Some(PRICE), landed).await;
            match settled.result {
                Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                    assert_eq!(
                        tx,
                        TransactionHash::Solana(*signature_of(&settled.sent[0]).as_array()),
                        "{case}"
                    );
                    assert_eq!(network, Network::Solana, "{case}");
                }
                other => panic!("{case}: expected SettlementUnconfirmed, got {other:?}"),
            }
        }
    }

    /// A standard payment's transfer is a top-level instruction, which a
    /// confirmed transaction executed: its confirmation settles it, as
    /// before, without reading the landed transaction back. That includes a
    /// payment from payTo to itself, whose balance does not move.
    #[tokio::test]
    async fn a_standard_payment_still_settles_on_its_confirmation() {
        for to_itself in [false, true] {
            let (fee_payer, payer) = (Keypair::new(), Keypair::new());
            let pay_to = if to_itself {
                payer.pubkey()
            } else {
                Pubkey::new_unique()
            };
            let transaction = standard_payment(&fee_payer.pubkey(), &payer, &usdc(), &pay_to);

            let settled =
                settle_on(Chain::new(usdc(), pay_to), fee_payer, &pay_to, transaction).await;

            let response = match settled.result {
                Ok(response) => response,
                Err(error) => panic!("to itself: {to_itself}: {error:?}"),
            };
            assert!(response.success, "to itself: {to_itself}: {response:?}");
            assert_eq!(
                response.transaction,
                Some(TransactionHash::Solana(
                    *signature_of(&settled.sent[0]).as_array()
                )),
                "to itself: {to_itself}"
            );
            assert_eq!(settled.reads, 0, "to itself: {to_itself}");
        }
    }
}
