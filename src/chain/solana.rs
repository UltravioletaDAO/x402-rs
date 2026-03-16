use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcSendTransactionConfig, RpcSimulateTransactionConfig};
use solana_commitment_config::CommitmentConfig;
use solana_sdk::instruction::CompiledInstruction;
use solana_sdk::pubkey;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status_client_types::{
    option_serializer::OptionSerializer, UiInnerInstructions, UiInstruction,
};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing_core::Level;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::Network;
use crate::types::{
    Base64Bytes, ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, PaymentRequirements,
    SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
    SupportedPaymentKindsResponse, TokenAmount, TransactionHash, VerifyRequest, VerifyResponse,
};
use crate::types::{Scheme, X402Version};

const ATA_PROGRAM_PUBKEY: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

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
            Network::XrplEvm => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
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
            Network::Near => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::NearTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::Stellar => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
            Network::StellarTestnet => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
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
    /// This enables smart wallet support (Squads, Crossmint, SWIG, etc.) where the token
    /// transfer is executed via Cross-Program Invocation rather than as a top-level instruction.
    fn find_transfer_in_inner_instructions(
        &self,
        inner_instructions: &[UiInnerInstructions],
        transaction: &VersionedTransaction,
        requirements: &PaymentRequirements,
    ) -> Result<TransferCheckedInstruction, FacilitatorLocalError> {
        let static_keys = transaction.message.static_account_keys();
        let fee_payer_pubkey = self.keypair.pubkey();
        let asset_address: SolanaAddress = requirements.asset.clone().try_into()?;
        let pay_to_address: SolanaAddress = requirements.pay_to.clone().try_into()?;

        // Derive expected destination ATA for both token programs
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

                // Resolve program ID from the transaction's account keys
                let program_id = match static_keys.get(compiled.program_id_index as usize) {
                    Some(pk) => *pk,
                    None => continue,
                };

                // Only look at spl_token and spl_token_2022 programs
                if program_id != spl_token::ID && program_id != spl_token_2022::ID {
                    continue;
                }

                // Decode bs58 instruction data
                let data = match bs58::decode(&compiled.data).into_vec() {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                // TransferChecked discriminator = 12, needs at least 10 bytes (1 + 8 + 1)
                if data.is_empty() || data[0] != 12 || data.len() < 10 {
                    continue;
                }

                // Parse amount (u64 LE) and decimals (u8)
                let mut amount_buf = [0u8; 8];
                amount_buf.copy_from_slice(&data[1..9]);
                let amount = u64::from_le_bytes(amount_buf);
                let decimals = data[9];

                // Resolve account keys: source(0), mint(1), destination(2), authority(3)
                if compiled.accounts.len() < 4 {
                    continue;
                }
                let resolve = |idx: usize| -> Option<Pubkey> {
                    static_keys.get(compiled.accounts[idx] as usize).copied()
                };
                let (source, mint, destination, authority) =
                    match (resolve(0), resolve(1), resolve(2), resolve(3)) {
                        (Some(s), Some(m), Some(d), Some(a)) => (s, m, d, a),
                        _ => continue,
                    };

                // Validate: mint must match expected asset
                if mint != asset_address.pubkey {
                    continue;
                }

                // Validate: destination must be the correct ATA
                if destination != expected_ata_spl && destination != expected_ata_2022 {
                    continue;
                }

                // Validate: amount must match requirements
                let instruction_amount: TokenAmount = amount.into();
                let requirements_amount: TokenAmount = requirements.max_amount_required;
                if instruction_amount != requirements_amount {
                    continue;
                }

                // Security: authority must not be the fee payer
                if authority == fee_payer_pubkey {
                    return Err(FacilitatorLocalError::DecodingError(
                        "invalid_exact_svm_payload_inner_transfer_fee_payer_is_authority".to_string(),
                    ));
                }

                // Ensure exactly ONE matching TransferChecked
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

        found.ok_or_else(|| {
            FacilitatorLocalError::DecodingError(
                "no_valid_transfer_in_inner_instructions".to_string(),
            )
        })
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

    async fn verify_transfer(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerifyTransferResult, FacilitatorLocalError> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;

        // Assert valid payment START
        let payment_payload = match &payload.payload {
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

        // 4. Simulate the transaction (with our signature added)
        // Enable inner_instructions to support smart wallet CPI detection (Path 2)
        let tx = TransactionInt::new(transaction.clone()).sign(&self.keypair)?;
        let cfg = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: false,
            commitment: Some(CommitmentConfig::confirmed()),
            encoding: None,
            accounts: None,
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

        // 5. Determine transfer instruction via Path 1 or Path 2
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
                            "no_valid_transfer_found_in_top_level_or_inner_instructions".to_string(),
                        ));
                    }
                }
            }
        };

        let payer: SolanaAddress = transfer_instruction.authority.into();
        Ok(VerifyTransferResult { payer, transaction })
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
    /// the RPC and verify it transferred sufficient USDC.
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
        let mut total_credit: u64 = 0;
        let mut payer_pubkey: Option<Pubkey> = None;

        for post_bal in post_balances {
            // Filter by mint
            if post_bal.mint != asset_str {
                continue;
            }

            let post_amount: u64 = post_bal
                .ui_token_amount
                .amount
                .parse()
                .unwrap_or(0);

            // Find matching pre-balance
            let pre_amount: u64 = pre_balances
                .iter()
                .find(|p| p.account_index == post_bal.account_index && p.mint == asset_str)
                .map(|p| p.ui_token_amount.amount.parse().unwrap_or(0))
                .unwrap_or(0);

            let diff = post_amount.saturating_sub(pre_amount);
            if diff > 0 {
                total_credit += diff;
                tracing::debug!(
                    account_index = post_bal.account_index,
                    credit = diff,
                    owner = ?post_bal.owner,
                    "Found USDC credit in settlement transaction"
                );
            }

            // Track the source (debit) as the payer
            if pre_amount > post_amount {
                if let OptionSerializer::Some(ref owner) = post_bal.owner {
                    if let Ok(pk) = Pubkey::from_str(owner) {
                        payer_pubkey = Some(pk);
                    }
                }
            }
        }

        // Also check debits to find the payer
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
    async fn settle_settlement_account(
        &self,
        payload: &crate::types::SettlementAccountPayload,
        requirements: &PaymentRequirements,
    ) -> Result<SettleResponse, FacilitatorLocalError> {
        // Step 1: Verify the on-chain transaction
        let verification = self
            .verify_settlement_account(payload, requirements)
            .await?;

        // Step 2: If settleSecretKey is provided, sweep funds from settlement account to payTo
        if let Some(ref secret_key_str) = payload.settle_secret_key {
            return self
                .sweep_settlement_account(secret_key_str, payload, requirements, &verification)
                .await;
        }

        // No secret key: funds already at payTo, return original tx signature
        tracing::info!(
            network = %self.network(),
            tx_signature = %verification.tx_signature,
            "Settlement account: no sweep needed (no settleSecretKey), returning original tx"
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
        let secret_bytes = solana_sdk::bs58::decode(secret_key_str)
            .into_vec()
            .map_err(|e| {
                FacilitatorLocalError::DecodingError(format!("invalid settleSecretKey: {e}"))
            })?;
        let settlement_keypair = Keypair::from_bytes(&secret_bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("invalid settleSecretKey keypair: {e}"))
        })?;
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

        // Check settlement account ATA balance
        let settlement_balance = self
            .rpc_client
            .get_token_account_balance(&settlement_ata)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!(
                    "failed to get settlement account balance: {e}"
                ))
            })?;
        let sweep_amount: u64 = settlement_balance.amount.parse().unwrap_or(0);

        if sweep_amount == 0 {
            // No balance to sweep - funds went directly to payTo
            tracing::info!(
                network = %self.network(),
                "Settlement account ATA has 0 balance, no sweep needed"
            );
            return Ok(SettleResponse {
                success: true,
                error_reason: None,
                payer: verification.payer.clone().into(),
                transaction: Some(TransactionHash::Solana(
                    *verification.tx_signature.as_array(),
                )),
                network: self.network(),
                proof_of_payment: None,
                extensions: None,
            });
        }

        tracing::info!(
            network = %self.network(),
            settlement_account = %settlement_pubkey,
            sweep_amount = sweep_amount,
            pay_to = %pay_to,
            "Sweeping settlement account to payTo"
        );

        let mut instructions = Vec::new();

        // 1. Create payTo ATA (idempotent - no-op if exists)
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

        // 2. TransferChecked from settlement ATA to payTo ATA
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

        // 3. Close settlement ATA (send rent to destination or facilitator)
        let rent_destination = payload
            .settlement_rent_destination
            .as_ref()
            .and_then(|s| Pubkey::from_str(s).ok())
            .unwrap_or(fee_payer);

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

        // Submit
        let tx_sig = self
            .rpc_client
            .send_and_confirm_transaction_with_spinner_and_config(
                &tx,
                CommitmentConfig::confirmed(),
                RpcSendTransactionConfig {
                    skip_preflight: false,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!(
                    "settlement account sweep failed: {e}"
                ))
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
        // Route: settlement account vs standard transaction
        if let ExactPaymentPayload::SolanaSettlementAccount(sa_payload) =
            &request.payment_payload.payload
        {
            let result = self
                .verify_settlement_account(sa_payload, &request.payment_requirements)
                .await?;
            return Ok(VerifyResponse::valid(result.payer.into()));
        }

        let verification = self.verify_transfer(request).await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        // Route: settlement account vs standard transaction
        if let ExactPaymentPayload::SolanaSettlementAccount(sa_payload) =
            &request.payment_payload.payload
        {
            return self
                .settle_settlement_account(sa_payload, &request.payment_requirements)
                .await;
        }

        // Standard flow: verify + co-sign + submit
        let verification = self.verify_transfer(request).await?;

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
            .send_and_confirm(&self.rpc_client, CommitmentConfig::confirmed())
            .await?;

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
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: None, // TODO: Add Solana token support
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
        rpc_client
            .send_transaction_with_config(
                &self.inner,
                RpcSendTransactionConfig {
                    skip_preflight: true,
                    max_retries: Some(5),
                    ..RpcSendTransactionConfig::default()
                },
            )
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))
    }

    pub async fn send_and_confirm(
        &self,
        rpc_client: &RpcClient,
        commitment_config: CommitmentConfig,
    ) -> Result<Signature, FacilitatorLocalError> {
        let tx_sig = self.send(rpc_client).await?;

        // Timeout for confirmation - configurable via environment variable
        // Default: 30 seconds (Solana blocks are ~400ms, 30s = ~75 blocks)
        let timeout_secs = std::env::var("SOLANA_CONFIRM_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30);
        let timeout_duration = Duration::from_secs(timeout_secs);

        let confirmation_future = async {
            loop {
                let confirmed = rpc_client
                    .confirm_transaction_with_commitment(&tx_sig, commitment_config)
                    .await
                    .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e}")))?;
                if confirmed.value {
                    return Ok::<Signature, FacilitatorLocalError>(tx_sig);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };

        match tokio::time::timeout(timeout_duration, confirmation_future).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(
                    tx_sig = %tx_sig,
                    timeout_secs = timeout_secs,
                    "Transaction confirmation timed out"
                );
                Err(FacilitatorLocalError::ContractCall(format!(
                    "Transaction confirmation timed out after {}s. TX may have been submitted: {}",
                    timeout_secs, tx_sig
                )))
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
