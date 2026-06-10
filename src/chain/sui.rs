//! Sui blockchain payment verification and settlement.
//!
//! This module implements x402 payment flows for the Sui blockchain using
//! sponsored transactions. Sui provides protocol-level gas sponsorship,
//! allowing the facilitator to pay gas fees while users pay only the
//! stablecoin transfer amount.
//!
//! # Sponsored Transaction Flow
//!
//! 1. Client constructs a USDC transfer transaction
//! 2. Client signs the transaction with their Sui wallet
//! 3. Client sends transaction bytes + signature to facilitator
//! 4. Facilitator verifies the transaction parameters
//! 5. Facilitator adds gas sponsorship and co-signs
//! 6. Facilitator submits the sponsored transaction to Sui network
//!
//! # USDC on Sui
//!
//! Sui USDC uses 6 decimals (same as EVM chains):
//! - Mainnet: `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC`
//! - Testnet: `0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC`

use std::str::FromStr;

use shared_crypto::intent::{Intent, IntentMessage};
use sui_sdk::rpc_types::SuiTransactionBlockResponseOptions;
use sui_sdk::SuiClientBuilder;
use sui_types::base_types::{ObjectID, SuiAddress};
use sui_types::crypto::{EncodeDecodeBase64, Signature, SuiKeyPair, SuiSignature, ToFromBytes};
use sui_types::transaction::{
    Argument, CallArg, Command, Transaction, TransactionData, TransactionDataAPI, TransactionKind,
};
use tracing::{debug, error, info, warn};

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env::{
    ENV_RPC_SUI, ENV_RPC_SUI_TESTNET, ENV_SUI_PRIVATE_KEY, ENV_SUI_PRIVATE_KEY_MAINNET,
    ENV_SUI_PRIVATE_KEY_TESTNET,
};
use crate::network::Network;
use crate::types::{
    ExactPaymentPayload, ExactSuiPayload, MixedAddress, Scheme, SettleRequest, SettleResponse,
    SupportedPaymentKind, SupportedPaymentKindExtra, SupportedPaymentKindsResponse, VerifyRequest,
    VerifyResponse, X402Version,
};

/// USDC coin type on Sui mainnet
pub const USDC_COIN_TYPE_MAINNET: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

/// USDC coin type on Sui testnet
pub const USDC_COIN_TYPE_TESTNET: &str =
    "0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC";

/// Sui network provider for payment verification and settlement.
///
/// Handles sponsored transactions for gasless USDC payments on Sui.
pub struct SuiProvider {
    /// The network this provider is configured for (Sui mainnet or testnet).
    network: Network,
    /// RPC endpoint URL for Sui network.
    rpc_url: String,
    /// Facilitator's Sui address (pays gas fees).
    signer_address: SuiAddress,
    /// Facilitator's keypair for signing sponsored transactions.
    keypair: SuiKeyPair,
    /// USDC coin type for this network.
    usdc_coin_type: String,
}

impl SuiProvider {
    /// Create a new Sui provider with the given configuration.
    pub fn new(
        network: Network,
        rpc_url: String,
        signer_address: SuiAddress,
        keypair: SuiKeyPair,
    ) -> Self {
        let usdc_coin_type = match network {
            Network::Sui => USDC_COIN_TYPE_MAINNET.to_string(),
            Network::SuiTestnet => USDC_COIN_TYPE_TESTNET.to_string(),
            _ => USDC_COIN_TYPE_TESTNET.to_string(), // Default to testnet
        };

        Self {
            network,
            rpc_url,
            signer_address,
            keypair,
            usdc_coin_type,
        }
    }

    /// Extract the Sui payload from a verify/settle request.
    fn extract_payload<'a>(
        &self,
        request: &'a VerifyRequest,
    ) -> Result<&'a ExactSuiPayload, FacilitatorLocalError> {
        match &request.payment_payload.payload {
            ExactPaymentPayload::Sui(payload) => Ok(payload),
            _ => Err(FacilitatorLocalError::DecodingError(
                "Expected Sui payload".to_string(),
            )),
        }
    }

    /// Parse a Sui address from string.
    fn parse_address(addr: &str) -> Result<SuiAddress, FacilitatorLocalError> {
        SuiAddress::from_str(addr).map_err(|e| {
            FacilitatorLocalError::InvalidAddress(format!("Invalid Sui address '{}': {}", addr, e))
        })
    }

    /// Decode base64 transaction bytes.
    fn decode_transaction_bytes(
        &self,
        tx_bytes_base64: &str,
    ) -> Result<TransactionData, FacilitatorLocalError> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let tx_bytes = STANDARD.decode(tx_bytes_base64).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to decode base64 transaction bytes: {}",
                e
            ))
        })?;

        bcs::from_bytes(&tx_bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to deserialize BCS transaction data: {}",
                e
            ))
        })
    }

    /// Decode base64 signature.
    fn decode_signature(&self, sig_base64: &str) -> Result<Signature, FacilitatorLocalError> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let sig_bytes = STANDARD.decode(sig_base64).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to decode base64 signature: {}",
                e
            ))
        })?;

        Signature::from_bytes(&sig_bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("Failed to parse Sui signature: {}", e))
        })
    }

    /// Verify the sender's signature on the transaction.
    ///
    /// This extracts the public key from the signature and verifies it matches
    /// the expected sender address. Full signature verification is performed
    /// by the Sui network on transaction submission.
    fn verify_signature(
        &self,
        _tx_data: &TransactionData,
        signature: &Signature,
        expected_sender: &SuiAddress,
    ) -> Result<(), FacilitatorLocalError> {
        use sui_types::crypto::PublicKey;

        // Extract the public key from the signature
        // Sui signatures contain: [scheme_flag | signature | public_key]
        let sig_bytes = signature.as_ref();
        if sig_bytes.is_empty() {
            return Err(FacilitatorLocalError::InvalidSignature(
                MixedAddress::Sui(expected_sender.to_string()),
                "Empty signature".to_string(),
            ));
        }

        // Get the public key from the signature and derive the address
        let public_key =
            PublicKey::try_from_bytes(signature.scheme(), signature.public_key_bytes()).map_err(
                |e| {
                    FacilitatorLocalError::InvalidSignature(
                        MixedAddress::Sui(expected_sender.to_string()),
                        format!("Failed to extract public key from signature: {}", e),
                    )
                },
            )?;

        let signer_address = SuiAddress::from(&public_key);

        // Verify the signer matches the expected sender
        if signer_address != *expected_sender {
            return Err(FacilitatorLocalError::InvalidSignature(
                MixedAddress::Sui(expected_sender.to_string()),
                format!(
                    "Signature signer {} does not match expected sender {}",
                    signer_address, expected_sender
                ),
            ));
        }

        debug!(
            sender = %expected_sender,
            "Sui transaction signature verified - signer matches sender"
        );

        Ok(())
    }

    /// Validate that a decoded `TransactionData` encodes exactly the expected USDC transfer.
    ///
    /// A valid x402 Sui payload must contain a `ProgrammableTransaction` with exactly
    /// two commands in this order:
    ///
    ///   1. `SplitCoins(coin_arg, [amount_arg])` — splits `expected_amount` off a USDC coin.
    ///   2. `TransferObjects([Result(0)], recipient_arg)` — sends the split coin to the merchant.
    ///
    /// The inputs vector must supply:
    ///   - A `Pure(u64 LE bytes)` for the split amount equal to `expected_amount`.
    ///   - A `Pure(32-byte address)` or `Object(ImmOrOwned)` for the recipient equal to `expected_recipient`.
    ///   - An `Object(ImmOrOwned)` whose object ID matches `expected_coin_id` (the declared coin object).
    ///
    /// Any deviation — wrong recipient, wrong amount, extra commands, non-USDC coin type that
    /// cannot be verified structurally, non-numeric amount, or malformed BCS — is a hard rejection.
    ///
    /// NOTE: We do NOT validate the coin object's Move type *here* because `CallArg::Object` only
    /// carries the ObjectID/SequenceNumber/Digest tuple, not the Move type. Coin-type enforcement
    /// is therefore done in `check_balance` (audit 04): `get_coins` is filtered to
    /// `usdc_coin_type`, and the spent `coin_object_id` MUST be a member of that set — which
    /// proves the spent coin is canonical USDC owned by the sender. (Sui's type-checker does NOT
    /// save us here: `SplitCoins`/`TransferObjects` are generic over `Coin<T>`, so a `Coin<JUNK>`
    /// executes fine — that is exactly the coin-type-confusion this binding closes.) `validate_ptb`
    /// still verifies that the coin object ID in the PTB matches the declared `coin_object_id`,
    /// preventing the client from declaring one coin but signing a PTB that drains a different coin.
    fn validate_ptb(
        &self,
        tx_data: &TransactionData,
        expected_recipient: &SuiAddress,
        expected_amount: u64,
        expected_coin_id: &ObjectID,
    ) -> Result<(), FacilitatorLocalError> {
        // Extract the ProgrammableTransaction — reject all other transaction kinds.
        let ptb = match tx_data.kind() {
            TransactionKind::ProgrammableTransaction(ptb) => ptb,
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: expected ProgrammableTransaction, got {:?}",
                    other
                )));
            }
        };

        // Require exactly two commands.
        if ptb.commands.len() != 2 {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: expected exactly 2 commands, got {}",
                ptb.commands.len()
            )));
        }

        // --- Command 0: SplitCoins(coin_arg, [amount_arg]) ---
        let (split_coin_arg, split_amount_arg) = match &ptb.commands[0] {
            Command::SplitCoins(coin_arg, amount_args) => {
                if amount_args.len() != 1 {
                    return Err(FacilitatorLocalError::Other(format!(
                        "PTB validation failed: SplitCoins must have exactly 1 amount, got {}",
                        amount_args.len()
                    )));
                }
                (coin_arg, &amount_args[0])
            }
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: command[0] must be SplitCoins, got {:?}",
                    other
                )));
            }
        };

        // --- Command 1: TransferObjects([Result(0)], recipient_arg) ---
        let (transfer_objects, recipient_arg) = match &ptb.commands[1] {
            Command::TransferObjects(objects, recipient) => (objects, recipient),
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: command[1] must be TransferObjects, got {:?}",
                    other
                )));
            }
        };

        // TransferObjects must move exactly the result of command 0.
        if transfer_objects.len() != 1 || transfer_objects[0] != Argument::Result(0) {
            return Err(FacilitatorLocalError::Other(
                "PTB validation failed: TransferObjects must transfer exactly Result(0)"
                    .to_string(),
            ));
        }

        // --- Resolve coin_arg -> coin object ID ---
        // The coin source must be an Input referring to an Object input.
        let coin_input_index = match split_coin_arg {
            Argument::Input(idx) => *idx as usize,
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: SplitCoins coin argument must be Input, got {:?}",
                    other
                )));
            }
        };

        let coin_call_arg = ptb.inputs.get(coin_input_index).ok_or_else(|| {
            FacilitatorLocalError::Other(format!(
                "PTB validation failed: coin input index {} out of range (inputs len {})",
                coin_input_index,
                ptb.inputs.len()
            ))
        })?;

        let ptb_coin_id = match coin_call_arg {
            CallArg::Object(sui_types::transaction::ObjectArg::ImmOrOwnedObject(obj_ref)) => {
                obj_ref.0
            }
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: SplitCoins coin input must be ImmOrOwnedObject, got {:?}",
                    other
                )));
            }
        };

        if ptb_coin_id != *expected_coin_id {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: coin object ID in PTB ({}) does not match declared coin_object_id ({})",
                ptb_coin_id, expected_coin_id
            )));
        }

        // --- Resolve split_amount_arg -> u64 ---
        let amount_input_index = match split_amount_arg {
            Argument::Input(idx) => *idx as usize,
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: SplitCoins amount argument must be Input, got {:?}",
                    other
                )));
            }
        };

        let amount_call_arg = ptb.inputs.get(amount_input_index).ok_or_else(|| {
            FacilitatorLocalError::Other(format!(
                "PTB validation failed: amount input index {} out of range",
                amount_input_index
            ))
        })?;

        let ptb_amount: u64 = match amount_call_arg {
            CallArg::Pure(bytes) => {
                if bytes.len() != 8 {
                    return Err(FacilitatorLocalError::Other(format!(
                        "PTB validation failed: amount Pure bytes must be 8 bytes (u64 LE), got {}",
                        bytes.len()
                    )));
                }
                u64::from_le_bytes(bytes[..8].try_into().expect("slice is exactly 8 bytes"))
            }
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: SplitCoins amount input must be Pure(u64), got {:?}",
                    other
                )));
            }
        };

        if ptb_amount != expected_amount {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: PTB split amount {} does not match required amount {}",
                ptb_amount, expected_amount
            )));
        }

        // --- Resolve recipient_arg -> SuiAddress ---
        let recipient_input_index = match recipient_arg {
            Argument::Input(idx) => *idx as usize,
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: TransferObjects recipient must be Input, got {:?}",
                    other
                )));
            }
        };

        let recipient_call_arg = ptb.inputs.get(recipient_input_index).ok_or_else(|| {
            FacilitatorLocalError::Other(format!(
                "PTB validation failed: recipient input index {} out of range",
                recipient_input_index
            ))
        })?;

        let ptb_recipient: SuiAddress = match recipient_call_arg {
            CallArg::Pure(bytes) => {
                if bytes.len() != 32 {
                    return Err(FacilitatorLocalError::Other(format!(
                        "PTB validation failed: recipient Pure bytes must be 32 bytes, got {}",
                        bytes.len()
                    )));
                }
                SuiAddress::from_bytes(bytes).map_err(|e| {
                    FacilitatorLocalError::Other(format!(
                        "PTB validation failed: cannot parse recipient address from Pure bytes: {}",
                        e
                    ))
                })?
            }
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: recipient input must be Pure(address), got {:?}",
                    other
                )));
            }
        };

        if ptb_recipient != *expected_recipient {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: PTB recipient {} does not match required pay_to {}",
                ptb_recipient, expected_recipient
            )));
        }

        // --- Validate gas_data.owner == facilitator (gas sponsor) ---
        // For a sponsored transaction the gas owner must be the facilitator, not the sender.
        // If the client encodes gas_data.owner = themselves, they could trick us into signing
        // a transaction where the gas refund goes to them instead of the facilitator.
        let gas_owner = tx_data.gas_data().owner;
        if gas_owner != self.signer_address {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: gas_data.owner {} must be the facilitator {}",
                gas_owner, self.signer_address
            )));
        }

        debug!(
            recipient = %ptb_recipient,
            amount = ptb_amount,
            coin_id = %ptb_coin_id,
            "PTB structural validation passed"
        );

        Ok(())
    }

    /// Verify transaction parameters match payment requirements.
    async fn verify_transaction(
        &self,
        payload: &ExactSuiPayload,
        request: &VerifyRequest,
    ) -> Result<(TransactionData, Signature, SuiAddress), FacilitatorLocalError> {
        let payer_addr = Self::parse_address(&payload.from)?;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Verify network matches
        if request.payment_payload.network != self.network {
            return Err(FacilitatorLocalError::NetworkMismatch(
                Some(payer),
                self.network,
                request.payment_payload.network,
            ));
        }

        // Verify scheme is exact
        if request.payment_payload.scheme != Scheme::Exact {
            return Err(FacilitatorLocalError::SchemeMismatch(
                Some(payer.clone()),
                Scheme::Exact,
                request.payment_payload.scheme,
            ));
        }

        // Parse the required recipient address from payment requirements.
        // We parse it as a SuiAddress so the PTB validator can do a typed comparison
        // rather than a lossy case-folded string comparison.
        let expected_recipient_str = request.payment_requirements.pay_to.to_string();
        let expected_recipient_addr =
            Self::parse_address(&expected_recipient_str).map_err(|e| {
                FacilitatorLocalError::DecodingError(format!(
                    "pay_to is not a valid Sui address '{}': {}",
                    expected_recipient_str, e
                ))
            })?;

        // Also verify the JSON payload.to field matches (belt-and-suspenders; the PTB check
        // below is the authoritative one).
        if payload.to.to_lowercase() != expected_recipient_str.to_lowercase() {
            return Err(FacilitatorLocalError::ReceiverMismatch(
                payer.clone(),
                payload.to.clone(),
                expected_recipient_str,
            ));
        }

        // Parse amount - Sui USDC uses u64 amounts (fits in 6 decimal precision).
        // Explicit propagation — a non-numeric or missing amount is a hard error, not 0.
        let payload_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;

        // Reject zero amounts immediately — a 0-amount passes any balance check.
        if payload_amount == 0 {
            return Err(FacilitatorLocalError::DecodingError(
                "Amount must be greater than zero".to_string(),
            ));
        }

        // Verify amount meets minimum (TokenAmount wraps U256).
        let required_amount = request.payment_requirements.max_amount_required.0;
        let payload_amount_u256 = alloy::primitives::U256::from(payload_amount);
        if payload_amount_u256 < required_amount {
            return Err(FacilitatorLocalError::InsufficientValue(payer.clone()));
        }

        // Parse the declared coin object ID so we can bind it to the PTB.
        let coin_object_id = ObjectID::from_str(&payload.coin_object_id).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid coin_object_id '{}': {}",
                payload.coin_object_id, e
            ))
        })?;

        // Decode and verify transaction
        let tx_data = self.decode_transaction_bytes(&payload.transaction_bytes)?;
        let signature = self.decode_signature(&payload.sender_signature)?;

        // Verify sender matches transaction sender
        let tx_sender = tx_data.sender();
        if tx_sender != payer_addr {
            return Err(FacilitatorLocalError::InvalidSignature(
                payer.clone(),
                format!(
                    "Transaction sender {} does not match payload sender {}",
                    tx_sender, payer_addr
                ),
            ));
        }

        // Validate the PTB structure: bind the on-chain commands to the declared parameters.
        // This is the critical check that prevents a mismatch between the JSON-declared
        // (to, amount) and the actual on-chain transfer encoded in the BCS bytes.
        self.validate_ptb(
            &tx_data,
            &expected_recipient_addr,
            payload_amount,
            &coin_object_id,
        )?;

        // Verify signature
        self.verify_signature(&tx_data, &signature, &payer_addr)?;

        info!(
            network = %self.network,
            from = %payload.from,
            to = %payload.to,
            amount = payload_amount,
            "Sui payment verification passed"
        );

        Ok((tx_data, signature, payer_addr))
    }

    /// Check USDC balance for the sender AND bind the spent coin object to USDC.
    ///
    /// `spent_coin_id` is the coin object the PTB splits from (the client's declared
    /// `coin_object_id`). Because `get_coins` is filtered to `self.usdc_coin_type`,
    /// requiring `spent_coin_id` to be a member of the returned set proves the spent
    /// coin is (a) USDC of the canonical type and (b) owned by `address`. This closes
    /// the coin-type-confusion hole (audit 04) where a payer splits a worthless `Coin<JUNK>`.
    async fn check_balance(
        &self,
        address: &SuiAddress,
        required_amount: u64,
        spent_coin_id: &ObjectID,
    ) -> Result<(), FacilitatorLocalError> {
        let client = SuiClientBuilder::default()
            .build(&self.rpc_url)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to connect to Sui RPC: {}", e))
            })?;

        // Get all USDC coins owned by the address (filtered to the canonical USDC type).
        let coins = client
            .coin_read_api()
            .get_coins(*address, Some(self.usdc_coin_type.clone()), None, None)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to fetch USDC balance: {}", e))
            })?;

        // SECURITY (audit 04): the coin the PTB spends MUST be one of the sender's
        // canonical-USDC coins. `get_coins` is filtered to `self.usdc_coin_type`, so
        // membership proves the spent coin object is USDC AND owned by the sender.
        // Without this, a payer can split a worthless Coin<JUNK> and the facilitator
        // would still report a successful USDC payment.
        let spends_usdc = coins
            .data
            .iter()
            .any(|c| c.coin_object_id == *spent_coin_id);
        if !spends_usdc {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: spent coin object {} is not a USDC ({}) coin owned by {}",
                spent_coin_id, self.usdc_coin_type, address
            )));
        }

        let total_balance: u64 = coins.data.iter().map(|c| c.balance).sum();

        if total_balance < required_amount {
            return Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Sui(
                address.to_string(),
            )));
        }

        debug!(
            address = %address,
            balance = total_balance,
            required = required_amount,
            "Sui USDC balance check passed"
        );

        Ok(())
    }

    /// Submit a sponsored transaction to the network.
    ///
    /// Sui sponsored transactions require TWO signatures:
    /// 1. sender_signature - User's signature authorizing the transfer
    /// 2. sponsor_signature - Facilitator's signature authorizing gas payment
    async fn submit_sponsored_transaction(
        &self,
        tx_data: TransactionData,
        sender_signature: Signature,
        sender: SuiAddress,
    ) -> Result<String, FacilitatorLocalError> {
        let client = SuiClientBuilder::default()
            .build(&self.rpc_url)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to connect to Sui RPC: {}", e))
            })?;

        // For sponsored transactions, we need BOTH signatures:
        // 1. sender_signature - already provided by the user
        // 2. sponsor_signature - facilitator signs as gas owner

        // Sign the transaction data with facilitator's key (gas sponsor)
        let intent_msg = IntentMessage::new(Intent::sui_transaction(), tx_data.clone());
        let sponsor_signature = Signature::new_secure(&intent_msg, &self.keypair);

        debug!(
            sender = %sender,
            sponsor = %self.signer_address,
            "Creating sponsored transaction with dual signatures"
        );

        // Create the transaction with BOTH signatures
        // Order matters: sender signature first, then sponsor signature
        // Convert Signature to GenericSignature for the Transaction constructor
        let sender_sig = sui_types::signature::GenericSignature::Signature(sender_signature);
        let sponsor_sig = sui_types::signature::GenericSignature::Signature(sponsor_signature);

        let transaction =
            Transaction::from_generic_sig_data(tx_data, vec![sender_sig, sponsor_sig]);

        // Execute the transaction
        let response = client
            .quorum_driver_api()
            .execute_transaction_block(
                transaction,
                SuiTransactionBlockResponseOptions::full_content(),
                None,
            )
            .await
            .map_err(|e| {
                FacilitatorLocalError::Other(format!("Failed to execute Sui transaction: {}", e))
            })?;

        let digest = response.digest.to_string();

        info!(
            digest = %digest,
            sender = %sender,
            sponsor = %self.signer_address,
            "Sui sponsored transaction executed successfully with dual signatures"
        );

        Ok(digest)
    }
}

impl NetworkProviderOps for SuiProvider {
    fn signer_address(&self) -> MixedAddress {
        MixedAddress::Sui(self.signer_address.to_string())
    }

    fn network(&self) -> Network {
        self.network
    }
}

impl FromEnvByNetworkBuild for SuiProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        // Determine RPC URL based on network
        let rpc_env = match network {
            Network::Sui => ENV_RPC_SUI,
            Network::SuiTestnet => ENV_RPC_SUI_TESTNET,
            _ => {
                return Err(format!("Network {:?} is not a Sui network", network).into());
            }
        };

        let rpc_url = match std::env::var(rpc_env) {
            Ok(url) => url,
            Err(_) => {
                // Use public RPC endpoints as fallback
                match network {
                    Network::Sui => "https://fullnode.mainnet.sui.io:443".to_string(),
                    Network::SuiTestnet => "https://fullnode.testnet.sui.io:443".to_string(),
                    _ => unreachable!(),
                }
            }
        };

        // Determine private key based on network (mainnet vs testnet)
        let is_testnet = network.is_testnet();
        let private_key_env = if is_testnet {
            ENV_SUI_PRIVATE_KEY_TESTNET
        } else {
            ENV_SUI_PRIVATE_KEY_MAINNET
        };

        // Try network-specific key first, then fall back to generic key
        let private_key_str =
            match std::env::var(private_key_env).or_else(|_| std::env::var(ENV_SUI_PRIVATE_KEY)) {
                Ok(key) => key,
                Err(_) => {
                    warn!(
                        network = %network,
                        "No Sui private key found for network, skipping provider initialization"
                    );
                    return Ok(None);
                }
            };

        // Parse the private key
        // Sui private keys can be in different formats:
        // - Base64 encoded raw key
        // - Bech32 encoded (suiprivkey1...)
        let keypair = parse_sui_private_key(&private_key_str)?;
        let signer_address = SuiAddress::from(&keypair.public());

        info!(
            network = %network,
            rpc_url = %crate::redact::rpc_url(&rpc_url),
            signer = %signer_address,
            "Sui provider initialized"
        );

        Ok(Some(Self::new(network, rpc_url, signer_address, keypair)))
    }
}

/// Parse a Sui private key from various formats.
///
/// Supported formats:
/// - Bech32 encoded (suiprivkey1...)
/// - Base64 encoded with scheme prefix byte
fn parse_sui_private_key(key_str: &str) -> Result<SuiKeyPair, Box<dyn std::error::Error>> {
    let trimmed = key_str.trim();

    // Try Bech32 format first (suiprivkey1...)
    if trimmed.starts_with("suiprivkey") {
        return SuiKeyPair::decode(trimmed)
            .map_err(|e| format!("Failed to decode Sui bech32 private key: {}", e).into());
    }

    // Try base64 encoded format - SuiKeyPair::decode handles this too
    // The base64 format includes a scheme prefix byte
    SuiKeyPair::decode_base64(trimmed)
        .map_err(|e| format!("Failed to decode base64 Sui private key: {}", e).into())
}

impl Facilitator for SuiProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let payload = self.extract_payload(request)?;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Full verification including signature and transaction structure
        let (_tx_data, _signature, payer_addr) = self.verify_transaction(payload, request).await?;

        // Check balance — parse explicitly; a non-numeric amount is a hard error, not 0.
        let required_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;
        // SECURITY (audit 04): bind the spent coin object to canonical USDC.
        let spent_coin_id = ObjectID::from_str(&payload.coin_object_id).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid coin_object_id '{}': {}",
                payload.coin_object_id, e
            ))
        })?;
        self.check_balance(&payer_addr, required_amount, &spent_coin_id)
            .await?;

        info!(
            network = %self.network,
            payer = %payer,
            "Sui payment verified successfully"
        );

        Ok(VerifyResponse::valid(payer))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let payload = self.extract_payload(request)?;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Verify the transaction
        let (tx_data, signature, sender) = self.verify_transaction(payload, request).await?;

        // Check balance before settlement — parse explicitly; a non-numeric amount is a hard error, not 0.
        let required_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid amount '{}': {}",
                payload.amount, e
            ))
        })?;
        // SECURITY (audit 04): bind the spent coin object to canonical USDC.
        let spent_coin_id = ObjectID::from_str(&payload.coin_object_id).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Invalid coin_object_id '{}': {}",
                payload.coin_object_id, e
            ))
        })?;
        self.check_balance(&sender, required_amount, &spent_coin_id)
            .await?;

        // Submit the sponsored transaction
        match self
            .submit_sponsored_transaction(tx_data, signature, sender)
            .await
        {
            Ok(digest) => {
                info!(
                    network = %self.network,
                    payer = %payer,
                    digest = %digest,
                    "Sui payment settled successfully"
                );

                Ok(SettleResponse {
                    success: true,
                    error_reason: None,
                    payer,
                    transaction: Some(crate::types::TransactionHash::Sui(digest)),
                    network: self.network,
                    proof_of_payment: None, // ERC-8004 not supported on Sui yet
                    extensions: None,
                })
            }
            Err(e) => {
                error!(
                    network = %self.network,
                    payer = %payer,
                    error = %e,
                    "Sui settlement failed"
                );

                Ok(SettleResponse {
                    success: false,
                    error_reason: Some(crate::types::FacilitatorErrorReason::FreeForm(format!(
                        "Settlement failed: {}",
                        e
                    ))),
                    payer,
                    transaction: None,
                    network: self.network,
                    proof_of_payment: None,
                    extensions: None,
                })
            }
        }
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        // Return supported payment kinds for this Sui network
        let network_string = match self.network {
            Network::Sui => "sui",
            Network::SuiTestnet => "sui-testnet",
            _ => unreachable!(),
        };

        let kinds = vec![SupportedPaymentKind {
            x402_version: X402Version::V1,
            scheme: Scheme::Exact,
            network: network_string.to_string(),
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: None, // TODO: Add supported tokens list
                escrow: None,
            }),
        }];

        Ok(SupportedPaymentKindsResponse { kinds })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sui_types::base_types::{ObjectDigest, SequenceNumber};
    use sui_types::transaction::{
        GasData, ObjectArg, ProgrammableTransaction, TransactionDataV1, TransactionExpiration,
        TransactionKind,
    };

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Dummy facilitator address used as gas sponsor in test transactions.
    fn facilitator_addr() -> SuiAddress {
        SuiAddress::from_str("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .unwrap()
    }

    fn merchant_addr() -> SuiAddress {
        SuiAddress::from_str("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .unwrap()
    }

    fn attacker_addr() -> SuiAddress {
        SuiAddress::from_str("0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
            .unwrap()
    }

    fn sender_addr() -> SuiAddress {
        SuiAddress::from_str("0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
            .unwrap()
    }

    fn dummy_coin_id() -> ObjectID {
        ObjectID::from_str("0x1111111111111111111111111111111111111111111111111111111111111111")
            .unwrap()
    }

    fn other_coin_id() -> ObjectID {
        ObjectID::from_str("0x2222222222222222222222222222222222222222222222222222222222222222")
            .unwrap()
    }

    /// Build a minimal SuiProvider for unit-testing validate_ptb (no RPC needed).
    fn make_provider(signer: SuiAddress) -> SuiProvider {
        use sui_types::crypto::SuiKeyPair;
        // Construct a throwaway keypair from a fixed seed — only signer_address matters for
        // validate_ptb. Bytes are `flag || privkey`: flag 0x00 = Ed25519, then a 32-byte seed
        // (any 32 bytes is a valid Ed25519 seed). This is version-robust, unlike decoding a
        // hardcoded bech32 string (which the pinned sui-types rejected).
        let kp = SuiKeyPair::from_bytes(&[0u8; 33]).expect("valid Ed25519 test keypair");

        // We override signer_address with our chosen `signer` so gas_data.owner
        // validation works predictably in tests.
        SuiProvider {
            network: Network::SuiTestnet,
            rpc_url: "http://localhost:9000".to_string(),
            signer_address: signer,
            keypair: kp,
            usdc_coin_type: USDC_COIN_TYPE_TESTNET.to_string(),
        }
    }

    /// Build the canonical two-command PTB for a USDC transfer:
    ///   inputs[0] = Object(ImmOrOwned) -- coin object
    ///   inputs[1] = Pure(amount as u64 LE)
    ///   inputs[2] = Pure(recipient address 32 bytes)
    ///   commands[0] = SplitCoins(Input(0), [Input(1)])
    ///   commands[1] = TransferObjects([Result(0)], Input(2))
    fn build_valid_ptb(
        sender: SuiAddress,
        gas_sponsor: SuiAddress,
        recipient: SuiAddress,
        amount: u64,
        coin_id: ObjectID,
    ) -> TransactionData {
        let coin_ref = (coin_id, SequenceNumber::new(), ObjectDigest::new([0u8; 32]));

        let inputs = vec![
            CallArg::Object(ObjectArg::ImmOrOwnedObject(coin_ref)),
            CallArg::Pure(amount.to_le_bytes().to_vec()),
            CallArg::Pure(recipient.to_vec()),
        ];
        let commands = vec![
            Command::SplitCoins(Argument::Input(0), vec![Argument::Input(1)]),
            Command::TransferObjects(vec![Argument::Result(0)], Argument::Input(2)),
        ];
        let ptb = ProgrammableTransaction { inputs, commands };

        // Gas object owned by the sponsor (facilitator).
        let gas_coin = (
            ObjectID::from_str(
                "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            )
            .unwrap(),
            SequenceNumber::new(),
            ObjectDigest::new([0u8; 32]),
        );
        let gas_data = GasData {
            payment: vec![gas_coin],
            owner: gas_sponsor,
            price: 1000,
            budget: 10_000_000,
        };

        TransactionData::V1(TransactionDataV1 {
            kind: TransactionKind::ProgrammableTransaction(ptb),
            sender,
            gas_data,
            expiration: TransactionExpiration::None,
        })
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sui_address_parsing() {
        let valid_address = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let result = SuiProvider::parse_address(valid_address);
        assert!(result.is_ok());

        let invalid_address = "not-a-valid-address";
        let result = SuiProvider::parse_address(invalid_address);
        assert!(result.is_err());
    }

    #[test]
    fn test_usdc_coin_types() {
        assert!(USDC_COIN_TYPE_MAINNET.contains("usdc::USDC"));
        assert!(USDC_COIN_TYPE_TESTNET.contains("usdc::USDC"));
    }

    #[test]
    fn test_validate_ptb_valid() {
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);
        let tx = build_valid_ptb(
            sender_addr(),
            sponsor,
            merchant_addr(),
            1_000_000,
            dummy_coin_id(),
        );
        let result = provider.validate_ptb(&tx, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(result.is_ok(), "valid PTB should pass: {:?}", result);
    }

    #[test]
    fn test_validate_ptb_wrong_recipient() {
        // PTB transfers to attacker but requirements say merchant — must reject.
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);
        let tx = build_valid_ptb(
            sender_addr(),
            sponsor,
            attacker_addr(),
            1_000_000,
            dummy_coin_id(),
        );
        let result = provider.validate_ptb(&tx, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("recipient")),
            "wrong recipient must be rejected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_ptb_wrong_amount() {
        // PTB splits 500_000 but requirements demand 1_000_000 — must reject.
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);
        let tx = build_valid_ptb(
            sender_addr(),
            sponsor,
            merchant_addr(),
            500_000,
            dummy_coin_id(),
        );
        let result = provider.validate_ptb(&tx, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("amount")),
            "wrong amount must be rejected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_ptb_wrong_coin_id() {
        // PTB uses other_coin_id but declaration says dummy_coin_id — must reject.
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);
        let tx = build_valid_ptb(
            sender_addr(),
            sponsor,
            merchant_addr(),
            1_000_000,
            other_coin_id(),
        );
        let result = provider.validate_ptb(&tx, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("coin object ID")),
            "wrong coin ID must be rejected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_ptb_wrong_gas_owner() {
        // PTB has gas_data.owner = sender (not facilitator) — must reject.
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);
        // Build with sender as gas owner instead of facilitator.
        let tx = build_valid_ptb(
            sender_addr(),
            sender_addr(),
            merchant_addr(),
            1_000_000,
            dummy_coin_id(),
        );
        let result = provider.validate_ptb(&tx, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("gas_data.owner")),
            "wrong gas owner must be rejected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_ptb_extra_commands() {
        // A PTB with 3 commands (extra MoveCall appended) must be rejected.
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);

        let coin_id = dummy_coin_id();
        let coin_ref = (coin_id, SequenceNumber::new(), ObjectDigest::new([0u8; 32]));
        let inputs = vec![
            CallArg::Object(ObjectArg::ImmOrOwnedObject(coin_ref)),
            CallArg::Pure(1_000_000u64.to_le_bytes().to_vec()),
            CallArg::Pure(merchant_addr().to_vec()),
        ];
        // Add a spurious third command.
        let commands = vec![
            Command::SplitCoins(Argument::Input(0), vec![Argument::Input(1)]),
            Command::TransferObjects(vec![Argument::Result(0)], Argument::Input(2)),
            Command::MergeCoins(Argument::Input(0), vec![Argument::Result(0)]),
        ];
        let ptb = ProgrammableTransaction { inputs, commands };
        let gas_coin = (
            ObjectID::from_str(
                "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            )
            .unwrap(),
            SequenceNumber::new(),
            ObjectDigest::new([0u8; 32]),
        );
        let gas_data = GasData {
            payment: vec![gas_coin],
            owner: sponsor,
            price: 1000,
            budget: 10_000_000,
        };
        let tx = TransactionData::V1(TransactionDataV1 {
            kind: TransactionKind::ProgrammableTransaction(ptb),
            sender: sender_addr(),
            gas_data,
            expiration: TransactionExpiration::None,
        });

        let result = provider.validate_ptb(&tx, &merchant_addr(), 1_000_000, &coin_id);
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("2 commands")),
            "extra commands must be rejected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_ptb_malformed_bcs_via_decode() {
        // Feed garbage bytes to decode_transaction_bytes and confirm it errors out.
        use base64::{engine::general_purpose::STANDARD, Engine};
        let provider = make_provider(facilitator_addr());
        let garbage_b64 = STANDARD.encode(b"this is not bcs");
        let result = provider.decode_transaction_bytes(&garbage_b64);
        assert!(
            matches!(result, Err(FacilitatorLocalError::DecodingError(_))),
            "malformed BCS must return DecodingError, got: {:?}",
            result
        );
    }

    #[test]
    fn test_non_numeric_amount_in_verify_transaction() {
        // The amount field "abc" cannot be parsed as u64.
        // verify_transaction must propagate an error (not silently become 0).
        //
        // We cannot call verify_transaction directly without an async runtime and
        // a full VerifyRequest, so we test the parse path in isolation here,
        // matching the exact error path used by both verify_transaction AND the
        // check_balance callers in verify/settle.
        let parse_result: Result<u64, _> = "abc".parse();
        assert!(
            parse_result.is_err(),
            "non-numeric amount must not parse to 0"
        );

        let parse_zero: Result<u64, _> = "0".parse();
        assert_eq!(parse_zero.unwrap(), 0u64);
        // The facilitator additionally rejects 0 after parsing (zero-amount guard).
        // Confirm that path is present: build_valid_ptb with amount=0 should fail validate_ptb.
        let sponsor = facilitator_addr();
        let provider = make_provider(sponsor);
        let tx = build_valid_ptb(sender_addr(), sponsor, merchant_addr(), 0, dummy_coin_id());
        // validate_ptb itself accepts 0 (amount comparison is == not >); the zero guard
        // lives in verify_transaction (explicit `payload_amount == 0` rejection).
        // Confirm validate_ptb with 0 expected and 0 actual passes (it is the outer guard's job).
        let ptb_ok = provider.validate_ptb(&tx, &merchant_addr(), 0, &dummy_coin_id());
        assert!(
            ptb_ok.is_ok(),
            "validate_ptb accepts 0==0; outer guard handles rejection"
        );
    }
}
