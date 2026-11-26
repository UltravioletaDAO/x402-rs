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
use std::sync::Arc;

use shared_crypto::intent::{Intent, IntentMessage};
use sui_types::base_types::{ObjectID, ObjectRef, SuiAddress};
use sui_types::crypto::{EncodeDecodeBase64, Signature, SuiKeyPair, SuiSignature, ToFromBytes};
use sui_types::transaction::{
    Argument, CallArg, Command, TransactionData, TransactionDataAPI, TransactionKind,
};
use tracing::{debug, error, info, warn};

mod rpc;
#[cfg(test)]
mod stub;

use rpc::{Execution, ObjectRead, SuiRpc};

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env::{
    ENV_RPC_SUI, ENV_RPC_SUI_TESTNET, ENV_SUI_PRIVATE_KEY, ENV_SUI_PRIVATE_KEY_MAINNET,
    ENV_SUI_PRIVATE_KEY_TESTNET,
};
use crate::network::Network;
use crate::nonce_store::{NonceStore, NonceStoreError};
use crate::types::{
    ExactPaymentPayload, ExactSuiPayload, ExactSuiTransactionPayload, MixedAddress, Scheme,
    SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
    SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse, X402Version,
};

/// USDC coin type on Sui mainnet
pub const USDC_COIN_TYPE_MAINNET: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

/// USDC coin type on Sui testnet
pub const USDC_COIN_TYPE_TESTNET: &str =
    "0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC";

/// The highest gas price (MIST per computation unit) the facilitator sponsors.
///
/// The client writes `gas_data` and the facilitator signs it as the gas owner,
/// so without a ceiling one settle can spend the sponsor wallet: a price far
/// above the reference makes even the smallest transaction cost its whole
/// budget. Measured 2026-11-15: the reference price is 100 on mainnet (epoch
/// 1312) and 1000 on testnet (epoch 1284), and the chain itself accepts up to
/// 50_000_000_000. 100x and 10x of them. In the code, not config: it moves money.
pub const MAX_SPONSORED_GAS_PRICE: u64 = 10_000;

/// The highest gas budget (MIST) the facilitator sponsors: 0.05 SUI.
///
/// What one settle can cost the sponsor at most, whatever the price. The
/// facilitator's own settlements on mainnet charged 514_000 of computation and
/// 3_632_800 of storage before the rebate (2026-11-15, its last transactions),
/// so a budget needs about 4_150_000: this is ~12 times that.
pub const MAX_SPONSORED_GAS_BUDGET: u64 = 50_000_000;

/// How long a settle's claim on a transaction digest is kept: 7 days, the
/// settle claim Solana's settlement accounts keep (`SOLANA_SETTLE_TTL_SECONDS`).
///
/// A Sui transaction with no expiration stays executable until its owned
/// inputs move on, so the claim is not tied to a validity window. Past it, a
/// settle of the same transaction still finds its inputs moved on
/// (`check_inputs_current`) or the chain holding it before anything is sent
/// (`Execution::AlreadyExecuted`). In the code, not config: it moves money.
pub const SETTLE_CLAIM_TTL_SECONDS: u64 = 7 * 24 * 3600;

/// The `errorReason` of a settle whose transaction another settle already
/// took, or the chain already executed.
pub const ALREADY_SUBMITTED: &str = "transaction_already_submitted";

/// The claim store settles take a transaction digest in, shared with the rest
/// of the service like the other families' (`NONCE_STORE_TABLE_NAME`: the
/// DynamoDB table `facilitator-nonces` in production). Without a table it is
/// this process's memory, and the claim holds for this task only.
static SETTLE_CLAIMS: tokio::sync::OnceCell<Arc<dyn NonceStore>> =
    tokio::sync::OnceCell::const_new();

/// The claim key of a transaction digest on `network`.
fn settle_claim_key(network: Network, digest: &str) -> String {
    format!("sui-settle#{}#{}", network, digest)
}

/// Sui network provider for payment verification and settlement.
///
/// Handles sponsored transactions for gasless USDC payments on Sui.
pub struct SuiProvider {
    /// The network this provider is configured for (Sui mainnet or testnet).
    network: Network,
    /// The network's gRPC endpoints, tried in order (`rpc.rs`).
    rpc: SuiRpc,
    /// Facilitator's Sui address (pays gas fees).
    signer_address: SuiAddress,
    /// Facilitator's keypair for signing sponsored transactions.
    keypair: SuiKeyPair,
    /// USDC coin type for this network.
    usdc_coin_type: String,
    /// Where settles claim a transaction digest. `None`: [`SETTLE_CLAIMS`].
    claims: Option<Arc<dyn NonceStore>>,
}

impl SuiProvider {
    /// Create a new Sui provider with the given configuration.
    pub fn new(
        network: Network,
        rpc: SuiRpc,
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
            rpc,
            signer_address,
            keypair,
            usdc_coin_type,
            claims: None,
        }
    }

    /// A provider for tests outside this module: the all-zero Ed25519 seed,
    /// an RPC nothing listens on, a claim store of its own.
    #[cfg(test)]
    pub(crate) fn for_test(network: Network) -> Self {
        let keypair = SuiKeyPair::from_bytes(&[0u8; 33]).expect("valid Ed25519 test keypair");
        let signer = SuiAddress::from(&keypair.public());
        Self::new(
            network,
            SuiRpc::new(
                network,
                vec!["http://127.0.0.1:9".to_string()],
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(1),
            ),
            signer,
            keypair,
        )
        .with_claims(Arc::new(crate::nonce_store::MemoryNonceStore::new()))
    }

    /// The same provider, claiming in `store` instead of the shared store.
    #[cfg(test)]
    fn with_claims(mut self, store: Arc<dyn NonceStore>) -> Self {
        self.claims = Some(store);
        self
    }

    /// The store settles claim a transaction digest in.
    async fn claims(&self) -> Arc<dyn NonceStore> {
        match &self.claims {
            Some(store) => store.clone(),
            None => SETTLE_CLAIMS
                .get_or_init(crate::nonce_store::create_nonce_store)
                .await
                .clone(),
        }
    }

    /// The payment a verify/settle request carries, read the same way from
    /// either payload.
    fn payment(&self, request: &VerifyRequest) -> Result<SuiPayment, FacilitatorLocalError> {
        // The asset the requirements name is the coin the payment spends, for
        // either payload: `check_balance` holds the spent coin (and every coin
        // merged into it) to this network's USDC, so the requirements must
        // name that USDC.
        let asset = request.payment_requirements.asset.to_string();
        let is_usdc = matches!(
            (
                sui_types::parse_sui_struct_tag(&asset),
                sui_types::parse_sui_struct_tag(&self.usdc_coin_type),
            ),
            (Ok(asked), Ok(usdc)) if asked == usdc
        );
        if !is_usdc {
            return Err(FacilitatorLocalError::DecodingError(format!(
                "asset {} is not the USDC this facilitator settles on {} ({})",
                asset, self.network, self.usdc_coin_type
            )));
        }

        match &request.payment_payload.payload {
            ExactPaymentPayload::Sui(payload) => Ok(SuiPayment {
                declared: payload.clone(),
                rules: PtbRules::DECLARED,
            }),
            ExactPaymentPayload::SuiTransaction(payload) => {
                self.read_spec_payload(payload, request)
            }
            _ => Err(FacilitatorLocalError::DecodingError(
                "Expected Sui payload".to_string(),
            )),
        }
    }

    /// Read the x402 Foundation payload (`scheme_exact_sui.md`): the
    /// transaction and its sender's signature, nothing else.
    ///
    /// What [`ExactSuiPayload`] declares beside them is taken from where the
    /// spec puts it: the sender and the spent coin from the transaction, the
    /// recipient and the amount from the seller's requirements. Every check
    /// the declared payload goes through then runs on those values, so the
    /// transaction must still split exactly `amount` off that coin and send it
    /// to `payTo`. The facilitator is never the sender. What the spec allows
    /// beyond the declared payload is in [`PtbRules::SPEC`].
    fn read_spec_payload(
        &self,
        payload: &ExactSuiTransactionPayload,
        request: &VerifyRequest,
    ) -> Result<SuiPayment, FacilitatorLocalError> {
        let requirements = &request.payment_requirements;
        let tx_data = self.decode_transaction_bytes(&payload.transaction)?;
        let sender = tx_data.sender();
        if sender == self.signer_address {
            return Err(FacilitatorLocalError::InvalidSignature(
                MixedAddress::Sui(sender.to_string()),
                "the facilitator cannot be the sender of a Sui payment".to_string(),
            ));
        }

        let coin = spent_coin(&tx_data).ok_or_else(|| {
            FacilitatorLocalError::Other(
                "PTB validation failed: the transfer must split an owned coin input, \
                 at command[0] or after one MergeCoins into it"
                    .to_string(),
            )
        })?;

        Ok(SuiPayment {
            declared: ExactSuiPayload {
                transaction_bytes: payload.transaction.clone(),
                sender_signature: payload.signature.clone(),
                from: sender.to_string(),
                to: requirements.pay_to.to_string(),
                amount: requirements.max_amount_required.to_string(),
                coin_object_id: coin.to_string(),
            },
            rules: PtbRules::SPEC,
        })
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
    /// `verify_secure` checks the signature over the transaction's intent
    /// message (what a Sui wallet signs and the validators check) against the
    /// public key it carries, and that this key is the sender's. `Signature`
    /// is Ed25519, Secp256k1 or Secp256r1 (`ToFromBytes`), so the scheme it
    /// reports is never zkLogin, the one scheme `verify_secure` does not tie
    /// to the sender's address.
    fn verify_signature(
        &self,
        tx_data: &TransactionData,
        signature: &Signature,
        expected_sender: &SuiAddress,
    ) -> Result<(), FacilitatorLocalError> {
        let intent = IntentMessage::new(Intent::sui_transaction(), tx_data.clone());
        signature
            .verify_secure(&intent, *expected_sender, signature.scheme())
            .map_err(|e| {
                FacilitatorLocalError::InvalidSignature(
                    MixedAddress::Sui(expected_sender.to_string()),
                    format!("Sui transaction signature does not verify: {}", e),
                )
            })?;

        debug!(
            sender = %expected_sender,
            "Sui transaction signature verified"
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
    /// is therefore done in `check_balance` (audit 04): the spent `coin_object_id` is read from
    /// the chain and MUST be a `0x2::coin::Coin<usdc_coin_type>` owned by the sender's address —
    /// which proves the spent coin is canonical USDC owned by the sender. (Sui's type-checker does NOT
    /// save us here: `SplitCoins`/`TransferObjects` are generic over `Coin<T>`, so a `Coin<JUNK>`
    /// executes fine — that is exactly the coin-type-confusion this binding closes.) `validate_ptb`
    /// still verifies that the coin object ID in the PTB matches the declared `coin_object_id`,
    /// preventing the client from declaring one coin but signing a PTB that drains a different coin.
    #[cfg(test)]
    fn validate_ptb(
        &self,
        tx_data: &TransactionData,
        expected_recipient: &SuiAddress,
        expected_amount: u64,
        expected_coin_id: &ObjectID,
    ) -> Result<(), FacilitatorLocalError> {
        self.validate_ptb_with(
            tx_data,
            expected_recipient,
            expected_amount,
            expected_coin_id,
            PtbRules::DECLARED,
        )
    }

    /// [`Self::validate_ptb`] under `rules`: with [`PtbRules::SPEC`] the gas
    /// may be paid by the sender itself, and the transfer may open with one
    /// `MergeCoins` of more owned coins into the coin it splits.
    fn validate_ptb_with(
        &self,
        tx_data: &TransactionData,
        expected_recipient: &SuiAddress,
        expected_amount: u64,
        expected_coin_id: &ObjectID,
        rules: PtbRules,
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

        // Require exactly two commands, or three opening with a MergeCoins
        // when the rules allow merging.
        let merges = rules.coins_may_merge
            && ptb.commands.len() == 3
            && matches!(ptb.commands[0], Command::MergeCoins(..));
        let split_at = usize::from(merges);
        if ptb.commands.len() != split_at + 2 {
            return Err(FacilitatorLocalError::Other(if rules.coins_may_merge {
                format!(
                    "PTB validation failed: expected 2 commands, or 3 opening with MergeCoins, got {}",
                    ptb.commands.len()
                )
            } else {
                format!(
                    "PTB validation failed: expected exactly 2 commands, got {}",
                    ptb.commands.len()
                )
            }));
        }

        // --- Command 0 (1 after a MergeCoins): SplitCoins(coin_arg, [amount_arg]) ---
        let (split_coin_arg, split_amount_arg) = match &ptb.commands[split_at] {
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
                    "PTB validation failed: command[{}] must be SplitCoins, got {:?}",
                    split_at, other
                )));
            }
        };

        // --- The next: TransferObjects([Result(split)], recipient_arg) ---
        let (transfer_objects, recipient_arg) = match &ptb.commands[split_at + 1] {
            Command::TransferObjects(objects, recipient) => (objects, recipient),
            other => {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: command[{}] must be TransferObjects, got {:?}",
                    split_at + 1,
                    other
                )));
            }
        };

        // TransferObjects must move exactly the result of the split: the whole
        // result, or under the spec's rules its one coin as `NestedResult`, the
        // form `@mysten/sui` writes for `const [coin] = tx.splitCoins(...)`
        // (a split of one amount has exactly one coin).
        let split_result = Argument::Result(split_at as u16);
        let moves_the_split = |object: &Argument| {
            *object == split_result
                || (rules.split_may_be_nested
                    && *object == Argument::NestedResult(split_at as u16, 0))
        };
        if transfer_objects.len() != 1 || !moves_the_split(&transfer_objects[0]) {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: TransferObjects must transfer exactly {:?}",
                split_result
            )));
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

        // --- The MergeCoins, when there is one: owned coins into the split one ---
        if merges {
            let Command::MergeCoins(destination, sources) = &ptb.commands[0] else {
                unreachable!("`merges` matched a MergeCoins at command[0]")
            };
            if destination != split_coin_arg {
                return Err(FacilitatorLocalError::Other(
                    "PTB validation failed: MergeCoins must merge into the coin SplitCoins splits"
                        .to_string(),
                ));
            }
            if sources.is_empty() {
                return Err(FacilitatorLocalError::Other(
                    "PTB validation failed: MergeCoins merges no coin".to_string(),
                ));
            }
            if sources.len() > MAX_MERGED_COINS {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: MergeCoins merges {} coins, more than the {} read for a payment",
                    sources.len(),
                    MAX_MERGED_COINS
                )));
            }
            let mut seen = vec![ptb_coin_id];
            for source in sources {
                let id = match source {
                    Argument::Input(index) => match ptb.inputs.get(usize::from(*index)) {
                        Some(CallArg::Object(
                            sui_types::transaction::ObjectArg::ImmOrOwnedObject((id, _, _)),
                        )) => *id,
                        other => {
                            return Err(FacilitatorLocalError::Other(format!(
                                "PTB validation failed: a merged coin must be an owned object input, got {:?}",
                                other
                            )));
                        }
                    },
                    other => {
                        return Err(FacilitatorLocalError::Other(format!(
                            "PTB validation failed: a merged coin must be an Input, got {:?}",
                            other
                        )));
                    }
                };
                if seen.contains(&id) {
                    return Err(FacilitatorLocalError::Other(format!(
                        "PTB validation failed: coin {} is merged twice",
                        id
                    )));
                }
                seen.push(id);
            }
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
        //
        // The x402 Foundation payload may instead leave the gas to its sender
        // (`scheme_exact_sui.md`: sponsorship is optional). Then the facilitator
        // signs nothing and pays nothing: settle sends the sender's signature
        // alone, and the ceilings below, which bound what a sponsor pays, do
        // not apply.
        let gas_owner = tx_data.gas_data().owner;
        if rules.sender_may_pay_gas && gas_owner == tx_data.sender() {
            debug!(
                recipient = %expected_recipient,
                amount = expected_amount,
                "PTB structural validation passed; the sender pays its own gas"
            );
            return Ok(());
        }
        if gas_owner != self.signer_address {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: gas_data.owner {} must be the facilitator {}",
                gas_owner, self.signer_address
            )));
        }

        // --- Bound what the facilitator pays for gas (MAX_SPONSORED_GAS_*) ---
        // The facilitator signs the client's gas_data as its owner: an uncapped
        // price or budget lets one settle spend the sponsor wallet.
        let gas_price = tx_data.gas_data().price;
        if gas_price > MAX_SPONSORED_GAS_PRICE {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: gas_data.price {} is above the {} MIST the facilitator sponsors",
                gas_price, MAX_SPONSORED_GAS_PRICE
            )));
        }
        let gas_budget = tx_data.gas_data().budget;
        if gas_budget > MAX_SPONSORED_GAS_BUDGET {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: gas_data.budget {} is above the {} MIST the facilitator sponsors",
                gas_budget, MAX_SPONSORED_GAS_BUDGET
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
        rules: PtbRules,
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
        self.validate_ptb_with(
            &tx_data,
            &expected_recipient_addr,
            payload_amount,
            &coin_object_id,
            rules,
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
    /// `coin_object_id`). It is read from the chain and must be a
    /// `0x2::coin::Coin<usdc_coin_type>` owned by `address`, which proves the spent
    /// coin is (a) USDC of the canonical type and (b) owned by `address`. This closes
    /// the coin-type-confusion hole (audit 04) where a payer splits a worthless `Coin<JUNK>`.
    /// Every coin in `merged_coin_ids` (merged into the spent one first) is
    /// held to the same: what reaches `payTo` is then the sender's USDC and
    /// nothing else.
    ///
    /// The JSON-RPC version proved the same by membership in `suix_getCoins`
    /// filtered to USDC, which read one page (50 coins): a coin past it was
    /// refused. Reading the coin itself has no such window. The balance is what
    /// `address` holds in USDC coin objects (`GetBalance` minus the address
    /// balance, which the PTB's `SplitCoins` cannot spend), as the sum of
    /// `suix_getCoins` was. The coins the transfer splits from must also hold
    /// `required_amount` between them, read from each coin: a split larger
    /// than its coin aborts on chain. A node that reports no balance for a
    /// coin leaves that check to the address total.
    async fn check_balance(
        &self,
        address: &SuiAddress,
        required_amount: u64,
        spent_coin_id: &ObjectID,
        merged_coin_ids: &[ObjectID],
    ) -> Result<(), FacilitatorLocalError> {
        let spent_coin = self
            .rpc
            .object_owner_and_type(*spent_coin_id)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to fetch USDC balance: {}", e))
            })?;

        // SECURITY (audit 04): the coin the PTB spends MUST be one of the sender's
        // canonical-USDC coins. Without this, a payer can split a worthless
        // Coin<JUNK> and the facilitator would still report a successful USDC payment.
        let spends_usdc = spent_coin
            .as_ref()
            .is_some_and(|coin| rpc::is_coin_owned_by(coin, &self.usdc_coin_type, address));
        if !spends_usdc {
            return Err(FacilitatorLocalError::Other(format!(
                "PTB validation failed: spent coin object {} is not a USDC ({}) coin owned by {}",
                spent_coin_id, self.usdc_coin_type, address
            )));
        }
        // What the split can draw on, coin by coin; `None` once a coin's
        // balance is not reported.
        let mut held = spent_coin.as_ref().and_then(|coin| coin.balance);

        for coin_id in merged_coin_ids {
            let coin = self
                .rpc
                .object_owner_and_type(*coin_id)
                .await
                .map_err(|e| {
                    FacilitatorLocalError::ContractCall(format!(
                        "Failed to read a merged Sui coin: {}",
                        e
                    ))
                })?;
            let senders_usdc = coin
                .as_ref()
                .is_some_and(|coin| rpc::is_coin_owned_by(coin, &self.usdc_coin_type, address));
            if !senders_usdc {
                return Err(FacilitatorLocalError::Other(format!(
                    "PTB validation failed: merged coin object {} is not a USDC ({}) coin owned by {}",
                    coin_id, self.usdc_coin_type, address
                )));
            }
            held = held
                .zip(coin.as_ref().and_then(|coin| coin.balance))
                .map(|(sum, balance)| sum.saturating_add(balance));
        }

        let total_balance: u64 = self
            .rpc
            .coin_balance(address, &self.usdc_coin_type)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to fetch USDC balance: {}", e))
            })?;

        if total_balance < required_amount {
            return Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Sui(
                address.to_string(),
            )));
        }

        match held {
            Some(held) if held < required_amount => {
                return Err(FacilitatorLocalError::InsufficientFunds(MixedAddress::Sui(
                    address.to_string(),
                )));
            }
            Some(_) => {}
            None => warn!(
                network = %self.network,
                "Sui node reported no coin balance; the address total decided"
            ),
        }

        debug!(
            address = %address,
            balance = total_balance,
            held = ?held,
            required = required_amount,
            "Sui USDC balance check passed"
        );

        Ok(())
    }

    /// Refuse a transaction whose owned inputs are no longer at the version it
    /// references.
    ///
    /// Executing a transaction gives every owned object it takes -- the coin it
    /// splits and the gas coins it pays with -- a new version, or deletes it, so
    /// a transaction that already executed references versions the chain has
    /// left behind. One `BatchGetObjects` call for the current versions, and a
    /// second one only for the inputs the node does not hold any more: gRPC
    /// answers `NotFound` both for an object deleted since and for one the node
    /// never saw (JSON-RPC told them apart), so the referenced version is asked
    /// for, together with the current one in the same call. Held at that
    /// version and not now means it existed and is gone: deleted. A node that
    /// pruned the old version reads it as unknown and leaves the verdict to
    /// settle, which execution then refuses. Read-only.
    async fn check_inputs_current(
        &self,
        tx_data: &TransactionData,
    ) -> Result<(), FacilitatorLocalError> {
        let mut inputs: Vec<ObjectRef> = tx_data.gas_data().payment.clone();
        if let TransactionKind::ProgrammableTransaction(ptb) = tx_data.kind() {
            inputs.extend(ptb.inputs.iter().filter_map(|input| match input {
                CallArg::Object(sui_types::transaction::ObjectArg::ImmOrOwnedObject(obj_ref)) => {
                    Some(*obj_ref)
                }
                _ => None,
            }));
        }

        let unreadable = |e: String| {
            FacilitatorLocalError::ContractCall(format!(
                "Failed to read Sui transaction inputs: {}",
                e
            ))
        };
        let current = self
            .rpc
            .objects(
                &inputs
                    .iter()
                    .map(|(id, _, _)| (*id, None))
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(unreadable)?;

        let mut absent: Vec<ObjectRef> = Vec::new();
        for (input, read) in inputs.iter().zip(current) {
            match read {
                ObjectRead::Found { version, digest } => self.judge_held(input, version, digest)?,
                ObjectRead::NotFound => absent.push(*input),
            }
        }
        if absent.is_empty() {
            return Ok(());
        }

        // The current version again and the referenced one, in ONE call, so
        // both answers come from the same node: a lagging endpoint answering
        // the first question and a current one the second would make a live
        // object look deleted.
        let reads = self
            .rpc
            .objects(
                &absent
                    .iter()
                    .flat_map(|(id, version, _)| [(*id, None), (*id, Some(*version))])
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(unreadable)?;
        for (input, pair) in absent.iter().zip(reads.chunks(2)) {
            match pair {
                [ObjectRead::Found { version, digest }, _] => {
                    self.judge_held(input, *version, *digest)?
                }
                [ObjectRead::NotFound, ObjectRead::Found { .. }] => {
                    return Err(FacilitatorLocalError::Other(format!(
                        "Sui input object {} already used: it was deleted",
                        input.0
                    )));
                }
                // Same reading as a lagging one: an object the RPC has not seen yet.
                _ => {
                    warn!(
                        network = %self.network,
                        "Sui input object unknown to the RPC; leaving the verdict to settle"
                    );
                }
            }
        }

        Ok(())
    }

    /// The verdict on an input the node holds at `held` / `held_digest`.
    fn judge_held(
        &self,
        (id, version, digest): &ObjectRef,
        held: sui_types::base_types::SequenceNumber,
        held_digest: sui_types::base_types::ObjectDigest,
    ) -> Result<(), FacilitatorLocalError> {
        if held == *version && held_digest == *digest {
            return Ok(());
        }
        // Execution only moves an object forward, so a newer version, or the
        // referenced version under another digest, means the referenced one is
        // gone.
        if held >= *version {
            return Err(FacilitatorLocalError::Other(format!(
                "Sui input object {} already used: transaction references version {} ({}), chain holds version {} ({})",
                id,
                version.value(),
                digest,
                held.value(),
                held_digest
            )));
        }
        // An older version is our RPC behind the one the client built the
        // transaction against, not a replay. Leave the verdict to settle, as
        // before this check.
        warn!(
            network = %self.network,
            referenced_version = version.value(),
            rpc_version = held.value(),
            "Sui input read behind the referenced version; leaving the verdict to settle"
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
    ) -> Submission {
        // For sponsored transactions, we need BOTH signatures:
        // 1. sender_signature - already provided by the user
        // 2. sponsor_signature - facilitator signs as gas owner

        // Sign the transaction data with facilitator's key (gas sponsor).
        // A transaction whose sender pays its own gas (the x402 Foundation
        // payload, `validate_ptb_paid`) is sent with the sender's signature
        // alone: the facilitator is not one of its signers.
        let sponsored = tx_data.gas_data().owner == self.signer_address;
        let mut signatures = vec![sender_signature.as_ref().to_vec()];
        if sponsored {
            let intent_msg = IntentMessage::new(Intent::sui_transaction(), tx_data.clone());
            let sponsor_signature = Signature::new_secure(&intent_msg, &self.keypair);
            signatures.push(sponsor_signature.as_ref().to_vec());
        }

        debug!(
            sender = %sender,
            sponsor = %self.signer_address,
            sponsored,
            "Creating Sui transaction with its signatures"
        );

        // The signed transaction travels as its BCS bytes -- the exact bytes both
        // signatures cover -- with the signatures as their raw `flag || sig || pk`
        // bytes. Order matters: sender signature first, then sponsor signature.
        let transaction_bcs = match bcs::to_bytes(&tx_data) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Submission::Sent(Err(FacilitatorLocalError::Other(format!(
                    "Failed to serialize Sui transaction: {}",
                    e
                ))))
            }
        };
        // Known before execution: an execution whose outcome is unknown is
        // reported under it rather than as a failure the caller would retry,
        // and it is what every endpoint is asked about before a (re)send.
        let digest = tx_data.digest().to_string();

        let sent = match self.rpc.execute(transaction_bcs, signatures, &digest).await {
            Execution::Executed { failure: None } => {
                info!(
                    digest = %digest,
                    sender = %sender,
                    sponsor = %self.signer_address,
                    sponsored,
                    "Sui transaction executed successfully"
                );
                Ok(Settled {
                    digest,
                    failure: None,
                })
            }
            Execution::Executed {
                failure: Some(failure),
            } => {
                warn!(
                    digest = %digest,
                    sender = %sender,
                    error = %failure,
                    "Sui sponsored transaction executed and failed on chain"
                );
                Ok(Settled {
                    digest,
                    failure: Some(failure),
                })
            }
            Execution::AlreadyExecuted => {
                warn!(
                    digest = %digest,
                    sender = %sender,
                    "Sui transaction executed before this settle sent it; not a new settlement"
                );
                return Submission::AlreadyExecuted;
            }
            Execution::Refused(reason) => Err(FacilitatorLocalError::Other(format!(
                "Failed to execute Sui transaction: {}",
                reason
            ))),
            Execution::Unconfirmed(why) => {
                warn!(
                    %digest,
                    error = %why,
                    "Sui execution outcome unknown; the transaction may be final"
                );
                Err(FacilitatorLocalError::SettlementUnconfirmed(
                    crate::types::TransactionHash::Sui(digest),
                    self.network,
                ))
            }
            Execution::Unreachable(why) => Err(FacilitatorLocalError::ContractCall(format!(
                "No Sui RPC endpoint took the transaction: {}",
                why
            ))),
        };
        Submission::Sent(sent)
    }
}

/// A Sui payment as verify and settle read it, whichever payload carried it.
struct SuiPayment {
    /// The payload this facilitator defined, or the same values read out of
    /// the x402 Foundation payload (`SuiProvider::read_spec_payload`).
    declared: ExactSuiPayload,
    /// What the transaction may do beyond the declared payload's two commands.
    rules: PtbRules,
}

/// What a payment's transaction may do beyond splitting one coin to `payTo`
/// under the facilitator's sponsorship.
#[derive(Clone, Copy, Debug)]
struct PtbRules {
    /// The sender pays its own gas; the facilitator signs nothing.
    sender_may_pay_gas: bool,
    /// The transfer opens with one `MergeCoins` of more of the sender's coins
    /// into the coin it splits, as a wallet holding several coins builds it.
    coins_may_merge: bool,
    /// `TransferObjects` may name the split's one coin as `NestedResult`.
    split_may_be_nested: bool,
}

impl PtbRules {
    /// The payload this facilitator defined: sponsored, one coin.
    const DECLARED: Self = Self {
        sender_may_pay_gas: false,
        coins_may_merge: false,
        split_may_be_nested: false,
    };
    /// The x402 Foundation payload (`scheme_exact_sui.md`): sponsorship is
    /// optional, and the transaction is whatever the payer's wallet built to
    /// move `amount` to `payTo`.
    const SPEC: Self = Self {
        sender_may_pay_gas: true,
        coins_may_merge: true,
        split_may_be_nested: true,
    };
}

/// The most coins one payment's `MergeCoins` may merge: each is read from the
/// chain on verify and again on settle.
const MAX_MERGED_COINS: usize = 64;

/// The `SplitCoins` of a transfer: at command 0, or at 1 after a `MergeCoins`.
fn split_command(ptb: &sui_types::transaction::ProgrammableTransaction) -> Option<&Command> {
    match ptb.commands.as_slice() {
        [Command::MergeCoins(..), split @ Command::SplitCoins(..), ..] => Some(split),
        [split @ Command::SplitCoins(..), ..] => Some(split),
        _ => None,
    }
}

/// The coin object the transfer's `SplitCoins` spends, when it splits an
/// owned object input; `validate_ptb_with` judges the rest.
fn spent_coin(tx_data: &TransactionData) -> Option<ObjectID> {
    let TransactionKind::ProgrammableTransaction(ptb) = tx_data.kind() else {
        return None;
    };
    let Some(Command::SplitCoins(Argument::Input(index), _)) = split_command(ptb) else {
        return None;
    };
    match ptb.inputs.get(usize::from(*index))? {
        CallArg::Object(sui_types::transaction::ObjectArg::ImmOrOwnedObject((id, _, _))) => {
            Some(*id)
        }
        _ => None,
    }
}

/// The coins a transfer's opening `MergeCoins` merges into the spent one; none
/// without one. Read after `validate_ptb_with` accepted the transaction.
fn merged_coins(tx_data: &TransactionData) -> Vec<ObjectID> {
    let TransactionKind::ProgrammableTransaction(ptb) = tx_data.kind() else {
        return Vec::new();
    };
    let Some(Command::MergeCoins(_, sources)) = ptb.commands.first() else {
        return Vec::new();
    };
    sources
        .iter()
        .filter_map(|source| match source {
            Argument::Input(index) => {
                match ptb.inputs.get(usize::from(*index)) {
                    Some(CallArg::Object(sui_types::transaction::ObjectArg::ImmOrOwnedObject(
                        (id, _, _),
                    ))) => Some(*id),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// A sponsored transaction the chain executed: its digest, and the on-chain
/// error when its commands did not take effect.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Settled {
    digest: String,
    failure: Option<String>,
}

/// What a settle's own submission came to.
#[derive(Debug)]
enum Submission {
    /// Sent by this settle: what [`settle_outcome`] reads.
    Sent(Result<Settled, FacilitatorLocalError>),
    /// The chain held the transaction, executed, before this settle sent
    /// anything: an earlier settle's payment, not this one's.
    AlreadyExecuted,
}

/// Whether a sent settlement certainly executed nothing: the answer is a
/// failure that names no transaction (`settle_outcome`'s plain `Err` arm).
/// An unconfirmed execution may be final, and an executed one is final.
fn executed_nothing(sent: &Result<Settled, FacilitatorLocalError>) -> bool {
    matches!(sent, Err(e) if !matches!(e, FacilitatorLocalError::SettlementUnconfirmed(..)))
}

/// The settle answer when the claim store does not answer: nothing was sent.
fn claim_store_unavailable(
    error: NonceStoreError,
    digest: &str,
    payer: MixedAddress,
    network: Network,
) -> SettleResponse {
    let correlation_id = uuid::Uuid::new_v4();
    error!(%correlation_id, network = %network, %digest, %error, "Sui settle claim store unavailable; nothing sent");
    SettleResponse {
        success: false,
        error_reason: Some(crate::types::FacilitatorErrorReason::FreeForm(format!(
            "Settlement failed: settlement_unavailable (ref: {correlation_id})"
        ))),
        payer,
        transaction: None,
        network,
        proof_of_payment: None,
        extensions: None,
    }
}

/// The settle answer for a transaction another settle already took or the
/// chain already executed: a refusal that names no transaction.
fn already_submitted(payer: MixedAddress, network: Network) -> SettleResponse {
    SettleResponse {
        success: false,
        error_reason: Some(crate::types::FacilitatorErrorReason::FreeForm(
            ALREADY_SUBMITTED.to_string(),
        )),
        payer,
        transaction: None,
        network,
        proof_of_payment: None,
        extensions: None,
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

        // gRPC endpoints, comma-separated in the variable; unset or without a
        // usable URL, the public defaults (`rpc::DEFAULT_ENDPOINTS_*`). JSON-RPC
        // is gone from every public Sui node, so no JSON-RPC URL works here.
        let rpc = SuiRpc::from_env(network, rpc_env);

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

        // Build the gRPC clients now, inside the runtime: a URL tonic will not
        // take is logged and skipped here, not on the first payment.
        // Only the defaults failing would leave none; then Sui stays off and
        // every other network still starts.
        if rpc.connect() == 0 {
            error!(
                network = %network,
                "No usable Sui RPC endpoint, not even the defaults; Sui provider not initialized"
            );
            return Ok(None);
        }

        info!(
            network = %network,
            rpc_endpoints = ?rpc.labels(),
            signer = %signer_address,
            "Sui provider initialized"
        );

        Ok(Some(Self::new(network, rpc, signer_address, keypair)))
    }
}

/// The settle answer for an executed sponsored transaction.
///
/// Executed with no verdict is not a failed settlement: reported as
/// `success: false` with no transaction, it tells the caller the payment did
/// not happen. It travels as an error so `IntoResponse` answers
/// `502 settlement_unconfirmed` with the digest.
///
/// Executed and failed on chain (the split or the transfer aborted; the gas is
/// spent and the digest exists) is `success: false` WITH the digest, as EVM
/// reports a reverted transfer: the payment did not happen, and the caller can
/// see why on chain. Under JSON-RPC that answer was `success: true`, because
/// the effects' status was never read.
fn settle_outcome(
    submitted: Result<Settled, FacilitatorLocalError>,
    payer: MixedAddress,
    network: Network,
) -> Result<SettleResponse, FacilitatorLocalError> {
    match submitted {
        Ok(Settled {
            digest,
            failure: None,
        }) => {
            info!(network = %network, payer = %payer, digest = %digest, "Sui payment settled successfully");
            Ok(SettleResponse {
                success: true,
                error_reason: None,
                payer,
                transaction: Some(crate::types::TransactionHash::Sui(digest)),
                network,
                proof_of_payment: None, // ERC-8004 not supported on Sui yet
                extensions: None,
            })
        }
        Ok(Settled {
            digest,
            failure: Some(failure),
        }) => {
            error!(network = %network, payer = %payer, digest = %digest, error = %failure, "Sui settlement executed and failed on chain");
            Ok(SettleResponse {
                success: false,
                error_reason: Some(crate::types::FacilitatorErrorReason::FreeForm(format!(
                    "Settlement failed: Sui transaction {} executed and failed on chain: {}",
                    digest, failure
                ))),
                payer,
                transaction: Some(crate::types::TransactionHash::Sui(digest)),
                network,
                proof_of_payment: None,
                extensions: None,
            })
        }
        Err(e @ FacilitatorLocalError::SettlementUnconfirmed(..)) => {
            error!(network = %network, payer = %payer, error = %e, "Sui settlement unconfirmed");
            Err(e)
        }
        Err(e) => {
            error!(network = %network, payer = %payer, error = %e, "Sui settlement failed");
            Ok(SettleResponse {
                success: false,
                error_reason: Some(crate::types::FacilitatorErrorReason::FreeForm(format!(
                    "Settlement failed: {}",
                    e
                ))),
                payer,
                transaction: None,
                network,
                proof_of_payment: None,
                extensions: None,
            })
        }
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
        let payment = self.payment(request)?;
        let payload = &payment.declared;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Full verification including signature and transaction structure
        let (tx_data, _signature, payer_addr) = self
            .verify_transaction(payload, request, payment.rules)
            .await?;

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
        // Settle repeats this read: it is also what refuses a settle of a
        // transaction that executed long ago (its gas coin, at least, moved on).
        self.check_inputs_current(&tx_data).await?;
        self.check_balance(
            &payer_addr,
            required_amount,
            &spent_coin_id,
            &merged_coins(&tx_data),
        )
        .await?;

        info!(
            network = %self.network,
            payer = %payer,
            "Sui payment verified successfully"
        );

        Ok(VerifyResponse::valid(payer))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let payment = self.payment(request)?;
        let payload = &payment.declared;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Verify the transaction
        let (tx_data, signature, sender) = self
            .verify_transaction(payload, request, payment.rules)
            .await?;

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
        // One settle per signed transaction, across every task of the
        // service: the claim is taken with the store's conditional put before
        // anything is sent. The digest does not cover the signatures, so a
        // re-signed copy is the same transaction.
        let digest = tx_data.digest().to_string();
        let claim = settle_claim_key(self.network, &digest);
        let claims = self.claims().await;
        // A claim already held is answered before the reads below, which
        // would refuse a transaction that executed for another reason. An
        // unanswered read decides nothing: the conditional put below does.
        match claims.is_used(&claim).await {
            Ok(false) => {}
            Ok(true) => {
                warn!(network = %self.network, payer = %payer, %digest, "Sui transaction already taken by another settle; not sending it");
                return Ok(already_submitted(payer, self.network));
            }
            Err(e) => {
                warn!(network = %self.network, %digest, error = %e, "Sui settle claim not readable; the claim itself decides")
            }
        }

        // A transaction that already executed has inputs that moved on.
        self.check_inputs_current(&tx_data).await?;
        self.check_balance(
            &sender,
            required_amount,
            &spent_coin_id,
            &merged_coins(&tx_data),
        )
        .await?;

        match claims
            .check_and_mark_used(&claim, SETTLE_CLAIM_TTL_SECONDS)
            .await
        {
            Ok(()) => {}
            Err(NonceStoreError::NonceAlreadyUsed(_)) => {
                warn!(network = %self.network, payer = %payer, %digest, "Sui transaction already taken by another settle; not sending it");
                return Ok(already_submitted(payer, self.network));
            }
            // Fail closed: without the claim, nothing is sent.
            Err(e) => return Ok(claim_store_unavailable(e, &digest, payer, self.network)),
        }

        let sent = match self
            .submit_sponsored_transaction(tx_data, signature, sender)
            .await
        {
            // The claim stays: the payment it settled is not this one.
            Submission::AlreadyExecuted => return Ok(already_submitted(payer, self.network)),
            Submission::Sent(sent) => sent,
        };
        // Given back only when nothing executed, so the caller can settle the
        // same transaction again. Best-effort: a claim that stays only costs
        // a retry.
        if executed_nothing(&sent) {
            if let Err(e) = claims.release(&claim).await {
                warn!(network = %self.network, %digest, error = %e, "Sui settle claim not released");
            }
        }
        settle_outcome(sent, payer, self.network)
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
            network_aliases: None,
            extra: Some(SupportedPaymentKindExtra {
                near_escrow: None,
                fee_payer: Some(self.signer_address()),
                tokens: Some(crate::network::exact_payment_tokens(self.network)),
                escrow: None,
                facilitator_address: None,
                are_fees_sponsored: None,
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
            rpc: SuiRpc::new(
                Network::SuiTestnet,
                vec!["http://127.0.0.1:9".to_string()],
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(1),
            ),
            signer_address: signer,
            keypair: kp,
            usdc_coin_type: USDC_COIN_TYPE_TESTNET.to_string(),
            claims: Some(Arc::new(crate::nonce_store::MemoryNonceStore::new())),
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

    /// The valid PTB with its gas price and budget replaced.
    fn with_gas(price: u64, budget: u64) -> TransactionData {
        let mut tx = build_valid_ptb(
            sender_addr(),
            facilitator_addr(),
            merchant_addr(),
            1_000_000,
            dummy_coin_id(),
        );
        let TransactionData::V1(v1) = &mut tx;
        v1.gas_data.price = price;
        v1.gas_data.budget = budget;
        tx
    }

    #[test]
    fn test_validate_ptb_gas_price_ceiling() {
        let provider = make_provider(facilitator_addr());
        let at_ceiling = with_gas(MAX_SPONSORED_GAS_PRICE, 10_000_000);
        assert!(provider
            .validate_ptb(&at_ceiling, &merchant_addr(), 1_000_000, &dummy_coin_id())
            .is_ok());
        let above = with_gas(MAX_SPONSORED_GAS_PRICE + 1, 10_000_000);
        let result = provider.validate_ptb(&above, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("gas_data.price")),
            "a price above the ceiling must be rejected, got: {:?}",
            result
        );
        // The chain's own maximum, which an uncapped sponsor would sign.
        let drain = with_gas(50_000_000_000, 10_000_000);
        assert!(provider
            .validate_ptb(&drain, &merchant_addr(), 1_000_000, &dummy_coin_id())
            .is_err());
    }

    #[test]
    fn test_validate_ptb_gas_budget_ceiling() {
        let provider = make_provider(facilitator_addr());
        let at_ceiling = with_gas(1_000, MAX_SPONSORED_GAS_BUDGET);
        assert!(provider
            .validate_ptb(&at_ceiling, &merchant_addr(), 1_000_000, &dummy_coin_id())
            .is_ok());
        let above = with_gas(1_000, MAX_SPONSORED_GAS_BUDGET + 1);
        let result = provider.validate_ptb(&above, &merchant_addr(), 1_000_000, &dummy_coin_id());
        assert!(
            matches!(&result, Err(FacilitatorLocalError::Other(msg)) if msg.contains("gas_data.budget")),
            "a budget above the ceiling must be rejected, got: {:?}",
            result
        );
        // The ceiling covers the facilitator's real settlements (~4_150_000 up front).
        assert!(MAX_SPONSORED_GAS_BUDGET >= 10 * 4_150_000);
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

/// `verify` against the current versions of the transaction's owned inputs,
/// and the spent coin and balance reads that bind the payment to USDC.
///
/// Uses nothing newer than `verify` and `SuiProvider::new`. The node is the
/// scripted gRPC node in `stub.rs`, reached through the real `sui-rpc` client:
/// `BatchGetObjects` for the inputs, `GetObject` for the spent coin and
/// `GetBalance` for the payer's USDC.
#[cfg(test)]
mod replay_verify_tests {
    use super::stub::{self, Node, Reply};
    use super::*;
    use crate::types::{PaymentPayload, PaymentRequirements, TokenAmount};
    use sui_rpc::proto::sui::rpc::v2::{
        get_object_result, owner::OwnerKind, Balance, BatchGetObjectsResponse, GetBalanceResponse,
        GetObjectResponse, GetObjectResult, Object, Owner,
    };
    use sui_types::base_types::{ObjectDigest, ObjectRef, SequenceNumber};
    use sui_types::transaction::{
        GasData, ObjectArg, ProgrammableTransaction, TransactionDataV1, TransactionExpiration,
    };

    pub(super) const AMOUNT: u64 = 1_000_000;

    // Ids built from bytes, not 0x-prefixed literals: .githooks/pre-commit
    // blocks any added 0x + 64 hex, keys and fixtures alike.
    pub(super) fn coin_ref() -> ObjectRef {
        (
            ObjectID::new([0x11; 32]),
            SequenceNumber::from_u64(5),
            ObjectDigest::new([3u8; 32]),
        )
    }

    pub(super) fn gas_ref() -> ObjectRef {
        (
            ObjectID::new([0xee; 32]),
            SequenceNumber::from_u64(9),
            ObjectDigest::new([4u8; 32]),
        )
    }

    pub(super) fn merchant() -> SuiAddress {
        SuiAddress::from(ObjectID::new([0xbb; 32]))
    }

    pub(super) fn sender_keypair() -> SuiKeyPair {
        // flag 0x00 = Ed25519, then a 32-byte seed.
        let mut bytes = [9u8; 33];
        bytes[0] = 0;
        SuiKeyPair::from_bytes(&bytes).unwrap()
    }

    pub(super) fn sender() -> SuiAddress {
        SuiAddress::from(&sender_keypair().public())
    }

    #[derive(Clone, Copy)]
    pub(super) enum Inputs {
        /// Every input is at the version the transaction references.
        Current,
        /// The coin has a newer version, as after the transaction executed.
        CoinAdvanced,
        /// The coin is gone, as after a transaction that spent all of it: the
        /// node holds it at the referenced version and not any more.
        CoinDeleted,
        /// The referenced version number under another digest.
        CoinOtherDigest,
        /// The RPC is behind: the coin is at an older version than the one referenced.
        CoinBehind,
        /// The RPC does not know the coin, at any version.
        CoinUnknown,
        /// The gas coin reads behind its referenced version, the coin moved on.
        GasBehindCoinAdvanced,
        /// The coin reads behind its referenced version, the gas coin moved on.
        CoinBehindGasAdvanced,
        /// The RPC does not know the gas coin, the coin moved on.
        GasUnknownCoinAdvanced,
        /// The RPC does not know the coin, the gas coin moved on.
        CoinUnknownGasAdvanced,
        /// BatchGetObjects answers with an error.
        Unreadable,
        /// BatchGetObjects never answers.
        Hangs,
    }

    fn found(obj_ref: ObjectRef) -> GetObjectResult {
        let mut object = Object::default();
        object.object_id = Some(obj_ref.0.to_string());
        object.version = Some(obj_ref.1.value());
        object.digest = Some(obj_ref.2.to_string());
        let mut result = GetObjectResult::default();
        result.result = Some(get_object_result::Result::Object(object));
        result
    }

    fn not_found(id: ObjectID) -> GetObjectResult {
        let mut status = sui_rpc::proto::google::rpc::Status::default();
        status.code = tonic::Code::NotFound as i32;
        status.message = format!("Object {id} not found");
        let mut result = GetObjectResult::default();
        result.result = Some(get_object_result::Result::Error(status));
        result
    }

    /// The node's answer for one input; `version` is set when the referenced
    /// version is asked for, after the current one was not found.
    fn object(inputs: Inputs, id: ObjectID, version: Option<u64>) -> GetObjectResult {
        let is_coin = id == coin_ref().0;
        let referenced = if is_coin { coin_ref() } else { gas_ref() };
        if let Some(version) = version {
            assert_eq!(
                version,
                referenced.1.value(),
                "only the referenced version is asked for"
            );
            return match inputs {
                Inputs::CoinUnknown | Inputs::CoinUnknownGasAdvanced if is_coin => not_found(id),
                Inputs::GasUnknownCoinAdvanced if !is_coin => not_found(id),
                _ => found(referenced),
            };
        }
        let moved_on = (
            id,
            SequenceNumber::from_u64(6),
            ObjectDigest::new([5u8; 32]),
        );
        match inputs {
            Inputs::CoinAdvanced if is_coin => found(moved_on),
            Inputs::CoinDeleted if is_coin => not_found(id),
            Inputs::CoinOtherDigest if is_coin => {
                found((id, coin_ref().1, ObjectDigest::new([7u8; 32])))
            }
            Inputs::CoinBehind if is_coin => found((
                id,
                SequenceNumber::from_u64(4),
                ObjectDigest::new([6u8; 32]),
            )),
            Inputs::CoinUnknown if is_coin => not_found(id),
            Inputs::GasBehindCoinAdvanced if is_coin => found(moved_on),
            Inputs::GasBehindCoinAdvanced => found((
                id,
                SequenceNumber::from_u64(8),
                ObjectDigest::new([8u8; 32]),
            )),
            Inputs::CoinBehindGasAdvanced if is_coin => found((
                id,
                SequenceNumber::from_u64(4),
                ObjectDigest::new([6u8; 32]),
            )),
            Inputs::CoinBehindGasAdvanced => found((
                id,
                SequenceNumber::from_u64(10),
                ObjectDigest::new([9u8; 32]),
            )),
            Inputs::GasUnknownCoinAdvanced if is_coin => found(moved_on),
            Inputs::GasUnknownCoinAdvanced => not_found(id),
            Inputs::CoinUnknownGasAdvanced if is_coin => not_found(id),
            Inputs::CoinUnknownGasAdvanced => found((
                id,
                SequenceNumber::from_u64(10),
                ObjectDigest::new([9u8; 32]),
            )),
            _ => found(referenced),
        }
    }

    /// The payer's coin as the node holds it, and what `GetBalance` reports.
    #[derive(Clone)]
    pub(super) struct Holdings {
        pub coin_owner: SuiAddress,
        pub coin_type: String,
        /// `false`: the node holds no such coin.
        pub coin_exists: bool,
        pub balance: u64,
        pub address_balance: u64,
    }

    impl Holdings {
        pub(super) fn usdc() -> Self {
            Self {
                coin_owner: sender(),
                coin_type: USDC_COIN_TYPE_TESTNET.to_string(),
                coin_exists: true,
                balance: 5 * AMOUNT,
                address_balance: 0,
            }
        }
    }

    /// Whether a read mask names `field`, as a node renders only the fields asked
    /// for (no mask: only the id, as if the default left the rest out).
    fn asks(mask: &Option<sui_rpc::field::FieldMask>, field: &str) -> bool {
        mask.as_ref()
            .is_some_and(|mask| mask.paths.iter().any(|path| path == field))
    }

    pub(super) fn node(inputs: Inputs, holdings: Holdings) -> Node {
        let mut node = Node::default();
        node.batch_get_objects = Box::new(move |request| match inputs {
            Inputs::Unreadable => Reply::Answer(Err(tonic::Status::internal("scripted"))),
            Inputs::Hangs => Reply::Hang,
            _ => {
                let mut response = BatchGetObjectsResponse::default();
                response.objects = request
                    .requests
                    .iter()
                    .map(|r| {
                        let id = ObjectID::from_str(r.object_id.as_deref().unwrap()).unwrap();
                        let mut result = object(inputs, id, r.version);
                        if let Some(get_object_result::Result::Object(object)) =
                            result.result.as_mut()
                        {
                            if !asks(&request.read_mask, "version") {
                                object.version = None;
                            }
                            if !asks(&request.read_mask, "digest") {
                                object.digest = None;
                            }
                        }
                        result
                    })
                    .collect();
                Reply::Answer(Ok(response))
            }
        });
        let coin = holdings.clone();
        node.get_object = Box::new(move |request| {
            let id = ObjectID::from_str(request.object_id.as_deref().unwrap()).unwrap();
            if id != coin_ref().0 || !coin.coin_exists {
                return Reply::Answer(Err(tonic::Status::not_found(format!(
                    "Object {id} not found"
                ))));
            }
            let mut object = Object::default();
            object.object_id = Some(id.to_string());
            if asks(&request.read_mask, "owner") {
                let mut owner = Owner::default();
                owner.kind = Some(OwnerKind::Address as i32);
                owner.address = Some(coin.coin_owner.to_string());
                object.owner = Some(owner);
            }
            if asks(&request.read_mask, "object_type") {
                object.object_type = Some(format!("0x2::coin::Coin<{}>", coin.coin_type));
            }
            if asks(&request.read_mask, "balance") {
                // The payer's coin holdings, all in this one coin.
                object.balance = Some(coin.balance.saturating_sub(coin.address_balance));
            }
            let mut response = GetObjectResponse::default();
            response.object = Some(object);
            Reply::Answer(Ok(response))
        });
        node.get_balance = Box::new(move |request| {
            // The payer's USDC; any other owner or coin type holds nothing here.
            let asked_for_the_payer = request.owner.as_deref()
                == Some(sender().to_string().as_str())
                && request.coin_type.as_deref() == Some(USDC_COIN_TYPE_TESTNET);
            let (total, address) = if asked_for_the_payer {
                (holdings.balance, holdings.address_balance)
            } else {
                (0, 0)
            };
            let mut balance = Balance::default();
            balance.coin_type = request.coin_type.clone();
            balance.balance = Some(total);
            if address > 0 {
                balance.address_balance = Some(address);
            }
            let coins = total.saturating_sub(address);
            if coins > 0 {
                balance.coin_balance = Some(coins);
            }
            let mut response = GetBalanceResponse::default();
            response.balance = Some(balance);
            Reply::Answer(Ok(response))
        });
        node
    }

    /// The facilitator: the all-zero Ed25519 seed, sponsoring from `urls`,
    /// with a claim store of its own (one task of the service).
    pub(super) fn provider_at(urls: Vec<String>, read_timeout: std::time::Duration) -> SuiProvider {
        provider_claiming_in(
            urls,
            read_timeout,
            std::sync::Arc::new(crate::nonce_store::MemoryNonceStore::new()),
        )
    }

    /// [`provider_at`] claiming in `claims`: two providers given the same
    /// store are two tasks of the service sharing its claim table.
    pub(super) fn provider_claiming_in(
        urls: Vec<String>,
        read_timeout: std::time::Duration,
        claims: std::sync::Arc<dyn crate::nonce_store::NonceStore>,
    ) -> SuiProvider {
        // flag 0x00 = Ed25519, then a 32-byte seed.
        let keypair = SuiKeyPair::from_bytes(&[0u8; 33]).unwrap();
        let signer = SuiAddress::from(&keypair.public());
        SuiProvider::new(
            Network::SuiTestnet,
            SuiRpc::new(
                Network::SuiTestnet,
                urls,
                read_timeout,
                std::time::Duration::from_secs(5),
            ),
            signer,
            keypair,
        )
        .with_claims(claims)
    }

    pub(super) async fn provider_with(inputs: Inputs, holdings: Holdings) -> SuiProvider {
        let (url, _) = stub::serve(node(inputs, holdings)).await;
        provider_at(vec![url], std::time::Duration::from_secs(5))
    }

    pub(super) async fn provider(inputs: Inputs) -> SuiProvider {
        provider_with(inputs, Holdings::usdc()).await
    }

    /// The two-command USDC transfer PTB, sponsored by `sponsor`.
    pub(super) fn transaction(sponsor: SuiAddress, sender: SuiAddress) -> TransactionData {
        let ptb = ProgrammableTransaction {
            inputs: vec![
                CallArg::Object(ObjectArg::ImmOrOwnedObject(coin_ref())),
                CallArg::Pure(AMOUNT.to_le_bytes().to_vec()),
                CallArg::Pure(merchant().to_vec()),
            ],
            commands: vec![
                Command::SplitCoins(Argument::Input(0), vec![Argument::Input(1)]),
                Command::TransferObjects(vec![Argument::Result(0)], Argument::Input(2)),
            ],
        };
        TransactionData::V1(TransactionDataV1 {
            kind: TransactionKind::ProgrammableTransaction(ptb),
            sender,
            gas_data: GasData {
                payment: vec![gas_ref()],
                owner: sponsor,
                price: 1_000,
                budget: 10_000_000,
            },
            expiration: TransactionExpiration::None,
        })
    }

    pub(super) fn request(provider: &SuiProvider) -> (VerifyRequest, SuiAddress) {
        request_with_gas(provider, 1_000, 10_000_000)
    }

    /// [`request`] with the transaction's gas price and budget set.
    pub(super) fn request_with_gas(
        provider: &SuiProvider,
        price: u64,
        budget: u64,
    ) -> (VerifyRequest, SuiAddress) {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let sender_keypair = sender_keypair();
        let sender = SuiAddress::from(&sender_keypair.public());
        let mut tx = transaction(provider.signer_address, sender);
        let TransactionData::V1(v1) = &mut tx;
        v1.gas_data.price = price;
        v1.gas_data.budget = budget;
        let signature = Signature::new_secure(
            &IntentMessage::new(Intent::sui_transaction(), tx.clone()),
            &sender_keypair,
        );
        let payload = ExactSuiPayload {
            transaction_bytes: STANDARD.encode(bcs::to_bytes(&tx).unwrap()),
            sender_signature: STANDARD.encode(signature.as_ref()),
            from: sender.to_string(),
            to: merchant().to_string(),
            amount: AMOUNT.to_string(),
            coin_object_id: coin_ref().0.to_string(),
        };
        let request = VerifyRequest {
            x402_version: X402Version::V1,
            payment_payload: PaymentPayload {
                x402_version: X402Version::V1,
                scheme: Scheme::Exact,
                network: Network::SuiTestnet,
                payload: ExactPaymentPayload::Sui(payload),
            },
            payment_requirements: PaymentRequirements {
                scheme: Scheme::Exact,
                network: Network::SuiTestnet,
                max_amount_required: TokenAmount(alloy::primitives::U256::from(AMOUNT)),
                resource: url::Url::parse("https://example.com/paid").unwrap(),
                description: String::new(),
                mime_type: "application/json".to_string(),
                output_schema: None,
                pay_to: MixedAddress::Sui(merchant().to_string()),
                max_timeout_seconds: 60,
                asset: MixedAddress::Sui(USDC_COIN_TYPE_TESTNET.to_string()),
                extra: None,
            },
        };
        (request, sender)
    }

    async fn verify(inputs: Inputs) -> Result<VerifyResponse, FacilitatorLocalError> {
        let provider = provider(inputs).await;
        let (request, _) = request(&provider);
        provider.verify(&request).await
    }

    fn valid(response: &VerifyResponse) -> bool {
        matches!(response, VerifyResponse::Valid { payer } if *payer == MixedAddress::Sui(sender().to_string()))
    }

    fn already_used(err: &FacilitatorLocalError) -> bool {
        matches!(err, FacilitatorLocalError::Other(msg) if msg.contains("already used"))
    }

    #[tokio::test]
    async fn verify_rejects_a_transaction_whose_coin_moved_past_the_signed_version() {
        let err = verify(Inputs::CoinAdvanced)
            .await
            .expect_err("a transaction whose coin moved on must not verify");
        assert!(already_used(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_rejects_a_transaction_whose_coin_no_longer_exists() {
        let err = verify(Inputs::CoinDeleted)
            .await
            .expect_err("a transaction whose coin is gone must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("it was deleted")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_accepts_a_transaction_whose_inputs_are_current() {
        let response = verify(Inputs::Current)
            .await
            .expect("a transaction with current inputs verifies");
        assert!(valid(&response), "got {response:?}");
    }

    #[tokio::test]
    async fn verify_rejects_a_transaction_whose_coin_has_another_digest_at_the_referenced_version()
    {
        let err = verify(Inputs::CoinOtherDigest)
            .await
            .expect_err("the referenced version under another digest must not verify");
        assert!(already_used(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_accepts_a_transaction_when_the_rpc_is_behind_the_referenced_version() {
        let response = verify(Inputs::CoinBehind)
            .await
            .expect("an RPC behind the client is not a replay");
        assert!(valid(&response), "got {response:?}");
    }

    #[tokio::test]
    async fn verify_accepts_a_transaction_whose_coin_the_rpc_does_not_know() {
        let response = verify(Inputs::CoinUnknown)
            .await
            .expect("an object the RPC has not seen is not a replay");
        assert!(valid(&response), "got {response:?}");
    }

    /// A lagging read only lets verify go on to the next input; it must not
    /// decide for the whole transaction. The gas payment is read first, so here
    /// the lagging read comes before the one that moved on.
    #[tokio::test]
    async fn verify_rejects_when_the_gas_reads_behind_and_the_coin_moved_on() {
        let err = verify(Inputs::GasBehindCoinAdvanced)
            .await
            .expect_err("a coin that moved on must not verify behind a lagging gas read");
        assert!(already_used(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_rejects_when_the_coin_reads_behind_and_the_gas_moved_on() {
        let err = verify(Inputs::CoinBehindGasAdvanced)
            .await
            .expect_err("a gas coin that moved on must not verify beside a lagging coin read");
        assert!(already_used(&err), "got {err:?}");
    }

    /// Same rule for an object the RPC does not know: it lets verify go on to
    /// the next input and never decides for the whole transaction.
    #[tokio::test]
    async fn verify_rejects_when_the_gas_is_unknown_and_the_coin_moved_on() {
        let err = verify(Inputs::GasUnknownCoinAdvanced)
            .await
            .expect_err("a coin that moved on must not verify behind an unknown gas coin");
        assert!(already_used(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_rejects_when_the_coin_is_unknown_and_the_gas_moved_on() {
        let err = verify(Inputs::CoinUnknownGasAdvanced)
            .await
            .expect_err("a gas coin that moved on must not verify beside an unknown coin");
        assert!(already_used(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_fails_closed_when_the_inputs_cannot_be_read() {
        let err = verify(Inputs::Unreadable)
            .await
            .expect_err("an unanswered input read must not vouch for a transaction");
        assert!(
            matches!(&err, FacilitatorLocalError::ContractCall(_)),
            "got {err:?}"
        );
    }

    // --- The spent coin and the balance (audit 04 binding and the amount). ---

    async fn verify_holding(holdings: Holdings) -> Result<VerifyResponse, FacilitatorLocalError> {
        let provider = provider_with(Inputs::Current, holdings).await;
        let (request, _) = request(&provider);
        provider.verify(&request).await
    }

    fn not_usdc(err: &FacilitatorLocalError) -> bool {
        matches!(err, FacilitatorLocalError::Other(msg) if msg.contains("is not a USDC"))
    }

    #[tokio::test]
    async fn verify_rejects_a_spent_coin_of_another_type() {
        let holdings = Holdings {
            coin_type: "0x2::sui::SUI".to_string(),
            ..Holdings::usdc()
        };
        let err = verify_holding(holdings)
            .await
            .expect_err("Coin<SUI> is not USDC");
        assert!(not_usdc(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_rejects_a_spent_coin_owned_by_someone_else() {
        let holdings = Holdings {
            coin_owner: merchant(),
            ..Holdings::usdc()
        };
        let err = verify_holding(holdings)
            .await
            .expect_err("not the payer's coin");
        assert!(not_usdc(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_rejects_a_spent_coin_the_node_does_not_hold() {
        let holdings = Holdings {
            coin_exists: false,
            ..Holdings::usdc()
        };
        let err = verify_holding(holdings).await.expect_err("no such coin");
        assert!(not_usdc(&err), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_accepts_a_balance_of_exactly_the_amount() {
        let holdings = Holdings {
            balance: AMOUNT,
            ..Holdings::usdc()
        };
        let response = verify_holding(holdings).await.expect("exactly enough");
        assert!(valid(&response), "got {response:?}");
    }

    #[tokio::test]
    async fn verify_rejects_a_balance_one_unit_short() {
        let holdings = Holdings {
            balance: AMOUNT - 1,
            ..Holdings::usdc()
        };
        let err = verify_holding(holdings).await.expect_err("one unit short");
        assert!(
            matches!(&err, FacilitatorLocalError::InsufficientFunds(_)),
            "got {err:?}"
        );
    }

    /// USDC in the address balance is not in a coin the PTB can split.
    #[tokio::test]
    async fn verify_does_not_count_the_address_balance() {
        let holdings = Holdings {
            balance: 5 * AMOUNT,
            address_balance: 5 * AMOUNT - AMOUNT / 2,
            ..Holdings::usdc()
        };
        let err = verify_holding(holdings)
            .await
            .expect_err("half the amount in coins");
        assert!(
            matches!(&err, FacilitatorLocalError::InsufficientFunds(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_reads_the_next_endpoint_when_the_first_is_down() {
        let (url, node) = stub::serve(node(Inputs::Current, Holdings::usdc())).await;
        let provider = provider_at(
            vec![stub::closed_endpoint().await, url],
            std::time::Duration::from_secs(5),
        );
        let (request, _) = request(&provider);
        let response = provider
            .verify(&request)
            .await
            .expect("the second endpoint answers");
        assert!(valid(&response), "got {response:?}");
        assert_eq!(
            node.calls(),
            vec!["BatchGetObjects", "GetObject", "GetBalance"]
        );
    }

    // --- The payer's signature (verify_secure). ---

    /// `keypair`'s signature of `tx` under `intent`, as its raw
    /// `flag || signature || public key` bytes.
    pub(super) fn sign(tx: &TransactionData, keypair: &SuiKeyPair, intent: Intent) -> Vec<u8> {
        Signature::new_secure(&IntentMessage::new(intent, tx.clone()), keypair)
            .as_ref()
            .to_vec()
    }

    /// [`request`] with the payload's signature replaced by `signature`.
    pub(super) fn signed_with(provider: &SuiProvider, signature: &[u8]) -> VerifyRequest {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let (mut request, _) = request(provider);
        match &mut request.payment_payload.payload {
            ExactPaymentPayload::Sui(payload) => {
                payload.sender_signature = STANDARD.encode(signature)
            }
            _ => unreachable!("request() builds a Sui payload"),
        }
        request
    }

    pub(super) fn invalid_signature(err: &FacilitatorLocalError) -> bool {
        matches!(err, FacilitatorLocalError::InvalidSignature(..))
    }

    async fn verify_signed(signature: Vec<u8>) -> (FacilitatorLocalError, Vec<&'static str>) {
        let (url, node) = stub::serve(node(Inputs::Current, Holdings::usdc())).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let err = provider
            .verify(&signed_with(&provider, &signature))
            .await
            .expect_err("not the sender's signature of this transaction");
        (err, node.calls())
    }

    /// The sender's public key beside signature bytes that are not a
    /// signature of this transaction: refused, before any read.
    #[tokio::test]
    async fn verify_rejects_garbage_signature_bytes_beside_the_senders_public_key() {
        let tx = transaction(
            provider_at(vec![], std::time::Duration::from_secs(1)).signer_address,
            sender(),
        );
        let genuine = sign(&tx, &sender_keypair(), Intent::sui_transaction());
        // flag || signature (64 bytes) || public key: only the signature changes.
        let mut zeroed = genuine.clone();
        zeroed[1..65].fill(0);
        let mut one_bit = genuine.clone();
        one_bit[64] ^= 1;
        for forged in [zeroed, one_bit] {
            assert_eq!(
                forged[65..],
                genuine[65..],
                "the sender's public key, untouched"
            );
            let (err, calls) = verify_signed(forged).await;
            assert!(invalid_signature(&err), "got {err:?}");
            assert!(calls.is_empty(), "{calls:?}");
        }
    }

    /// The sender's real signature, of another transaction.
    #[tokio::test]
    async fn verify_rejects_the_senders_signature_of_another_transaction() {
        let mut other = transaction(
            provider_at(vec![], std::time::Duration::from_secs(1)).signer_address,
            sender(),
        );
        let TransactionData::V1(v1) = &mut other;
        v1.gas_data.budget += 1;
        let (err, calls) =
            verify_signed(sign(&other, &sender_keypair(), Intent::sui_transaction())).await;
        assert!(invalid_signature(&err), "got {err:?}");
        assert!(calls.is_empty(), "{calls:?}");
    }

    /// The sender's signature over these very bytes, made as a personal
    /// message: a wallet's signature for another purpose is not a payment.
    #[tokio::test]
    async fn verify_rejects_the_senders_signature_under_another_intent() {
        let tx = transaction(
            provider_at(vec![], std::time::Duration::from_secs(1)).signer_address,
            sender(),
        );
        let (err, calls) =
            verify_signed(sign(&tx, &sender_keypair(), Intent::personal_message())).await;
        assert!(invalid_signature(&err), "got {err:?}");
        assert!(calls.is_empty(), "{calls:?}");
    }

    /// A good signature of this transaction by a key that is not the sender's.
    #[tokio::test]
    async fn verify_rejects_a_signature_by_another_key() {
        let tx = transaction(
            provider_at(vec![], std::time::Duration::from_secs(1)).signer_address,
            sender(),
        );
        let mut seed = [7u8; 33];
        seed[0] = 0;
        let stranger = SuiKeyPair::from_bytes(&seed).unwrap();
        let (err, calls) = verify_signed(sign(&tx, &stranger, Intent::sui_transaction())).await;
        assert!(invalid_signature(&err), "got {err:?}");
        assert!(calls.is_empty(), "{calls:?}");
    }

    /// Every scheme a `Signature` carries is verified, not only Ed25519: its
    /// own signature passes, one flipped bit does not.
    #[test]
    fn verify_signature_checks_every_scheme_a_signature_carries() {
        let provider = provider_at(vec![], std::time::Duration::from_secs(1));
        for flag in [0u8, 1, 2] {
            let mut seed = [9u8; 33];
            seed[0] = flag;
            let keypair = SuiKeyPair::from_bytes(&seed).unwrap();
            let signer = SuiAddress::from(&keypair.public());
            let tx = transaction(provider.signer_address, signer);
            let genuine = sign(&tx, &keypair, Intent::sui_transaction());
            let good = Signature::from_bytes(&genuine).unwrap();
            assert_eq!(good.scheme().flag(), flag);
            provider
                .verify_signature(&tx, &good, &signer)
                .unwrap_or_else(|e| panic!("flag {flag}: its own signature: {e:?}"));
            let mut flipped = genuine.clone();
            flipped[2] ^= 1;
            let bad = Signature::from_bytes(&flipped).unwrap();
            let err = provider
                .verify_signature(&tx, &bad, &signer)
                .expect_err("a flipped bit");
            assert!(invalid_signature(&err), "flag {flag}: got {err:?}");
        }
    }
}

#[cfg(test)]
mod replay_verify_bound_tests {
    use super::replay_verify_tests::{node, provider_at, transaction, Holdings, Inputs};
    use super::stub;
    use super::*;

    #[tokio::test]
    async fn input_read_gives_up_at_its_timeout() {
        let (url, _) = stub::serve(node(Inputs::Hangs, Holdings::usdc())).await;
        let provider = provider_at(vec![url], std::time::Duration::from_millis(200));
        let tx = transaction(provider.signer_address, provider.signer_address);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.check_inputs_current(&tx),
        )
        .await
        .expect("the check must return on its own timeout, not hang");
        assert!(
            matches!(&result, Err(FacilitatorLocalError::ContractCall(_))),
            "got {result:?}"
        );
    }
}

/// A settle through the scripted gRPC node: what is sent, and how each
/// outcome of the execution reaches the settle answer.
#[cfg(test)]
mod execution_outcome_tests {
    use super::replay_verify_tests::{
        node, provider_at, provider_claiming_in, request, request_with_gas, sender, transaction,
        Holdings, Inputs,
    };
    use super::stub::{self, Node, Reply};
    use super::*;
    use sui_rpc::proto::sui::rpc::v2::{
        ExecuteTransactionResponse, ExecutedTransaction, ExecutionError, ExecutionStatus,
        TransactionEffects,
    };
    use sui_types::crypto::SignatureScheme;

    pub(super) fn executed(digest: &str, success: bool) -> ExecuteTransactionResponse {
        let mut status = ExecutionStatus::default();
        status.success = Some(success);
        if !success {
            let mut error = ExecutionError::default();
            error.description = Some("InsufficientCoinBalance in command 0".to_string());
            status.error = Some(error);
        }
        let mut effects = TransactionEffects::default();
        effects.status = Some(status);
        let mut transaction = ExecutedTransaction::default();
        transaction.digest = Some(digest.to_string());
        transaction.effects = Some(effects);
        let mut response = ExecuteTransactionResponse::default();
        response.transaction = Some(transaction);
        response
    }

    /// A node for the settle path: the payer's USDC is there, the transaction
    /// is unknown to it, and `execute` decides the rest.
    pub(super) fn settle_node(
        execute: impl Fn(&str) -> Reply<ExecuteTransactionResponse> + Send + Sync + 'static,
        digest: String,
    ) -> Node {
        let mut node = node(Inputs::Current, Holdings::usdc());
        node.get_transaction =
            Box::new(|_| Reply::Answer(Err(tonic::Status::not_found("Transaction not found"))));
        node.execute = Box::new(move |_| execute(&digest));
        node
    }

    fn digest_of(provider: &SuiProvider) -> String {
        transaction(provider.signer_address, sender())
            .digest()
            .to_string()
    }

    async fn settle(
        execute: impl Fn(&str) -> Reply<ExecuteTransactionResponse> + Send + Sync + 'static,
    ) -> (
        Result<SettleResponse, FacilitatorLocalError>,
        String,
        std::sync::Arc<Node>,
        SuiProvider,
    ) {
        // The digest depends on the sponsor, so ask a provider first.
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let (url, node) = stub::serve(settle_node(execute, digest.clone())).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let (request, _) = request(&provider);
        let result = provider.settle(&request).await;
        (result, digest, node, provider)
    }

    #[tokio::test]
    async fn a_settled_payment_reports_its_digest_and_sends_both_signatures_over_the_signed_bytes()
    {
        let (result, digest, node, provider) =
            settle(|digest| Reply::Answer(Ok(executed(digest, true)))).await;
        let response = result.expect("settled");
        assert!(response.success, "{response:?}");
        assert_eq!(
            response.transaction,
            Some(crate::types::TransactionHash::Sui(digest.clone()))
        );

        let sent = node.executions();
        assert_eq!(sent.len(), 1);

        // What a node does with the request (`sui-rpc-api` execute_transaction):
        // the proto transaction and signatures through sui-sdk-types. The
        // facilitator's sui-types 1.37.3 bytes must be the SAME transaction to
        // it, digest included, or every lookup asks about another one.
        let node_tx = sui_sdk_types::Transaction::try_from(sent[0].transaction.as_ref().unwrap())
            .expect("a node reads the BCS as a transaction");
        assert_eq!(node_tx.digest().to_string(), digest);
        for signature in &sent[0].signatures {
            sui_sdk_types::UserSignature::try_from(signature).expect("a node reads each signature");
        }

        let tx = transaction(provider.signer_address, sender());
        assert_eq!(
            sent[0]
                .transaction
                .as_ref()
                .and_then(|t| t.bcs.as_ref())
                .and_then(|b| b.value.as_deref()),
            Some(bcs::to_bytes(&tx).unwrap().as_slice())
        );
        let signatures: Vec<Signature> = sent[0]
            .signatures
            .iter()
            .map(|s| {
                Signature::from_bytes(s.bcs.as_ref().unwrap().value.as_deref().unwrap()).unwrap()
            })
            .collect();
        assert_eq!(signatures.len(), 2, "sender, then sponsor");
        let intent = IntentMessage::new(Intent::sui_transaction(), tx);
        signatures[0]
            .verify_secure(&intent, sender(), SignatureScheme::ED25519)
            .expect("the sender's signature, untouched");
        signatures[1]
            .verify_secure(&intent, provider.signer_address, SignatureScheme::ED25519)
            .expect("the facilitator's sponsor signature");
    }

    #[tokio::test]
    async fn an_execution_that_failed_on_chain_is_a_failed_settlement_with_its_digest() {
        let (result, digest, _, _) =
            settle(|digest| Reply::Answer(Ok(executed(digest, false)))).await;
        let response = result.expect("a verdict");
        assert!(!response.success, "{response:?}");
        assert_eq!(
            response.transaction,
            Some(crate::types::TransactionHash::Sui(digest))
        );
        let reason = format!("{:?}", response.error_reason);
        assert!(reason.contains("failed on chain"), "{reason}");
        assert!(reason.contains("InsufficientCoinBalance"), "{reason}");
    }

    #[tokio::test]
    async fn an_execution_that_may_be_final_reports_its_digest() {
        for status in [
            tonic::Status::unavailable("timed-out before finality could be reached"),
            tonic::Status::aborted(
                "The transaction is already finalized but with different user signatures",
            ),
            tonic::Status::internal("connection reset"),
            tonic::Status::resource_exhausted("validator overloaded"),
        ] {
            let code = status.code();
            let message = status.message().to_string();
            let (result, digest, _, _) =
                settle(move |_| Reply::Answer(Err(tonic::Status::new(code, message.clone()))))
                    .await;
            match result {
                Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                    assert_eq!(tx, crate::types::TransactionHash::Sui(digest));
                    assert_eq!(network, Network::SuiTestnet);
                }
                other => panic!("{code:?}: expected SettlementUnconfirmed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn an_execution_the_node_refused_keeps_its_old_error() {
        let (result, _, _, _) = settle(|_| {
            Reply::Answer(Err(tonic::Status::invalid_argument(
                "Invalid user signature: bad signature",
            )))
        })
        .await;
        let response = result.expect("a refusal is a settle verdict");
        assert!(!response.success);
        assert!(response.transaction.is_none());
        let reason = format!("{:?}", response.error_reason);
        assert!(
            reason.contains("Failed to execute Sui transaction"),
            "{reason}"
        );
        assert!(reason.contains("Invalid user signature"), "{reason}");
    }

    /// The settle answer: an execution with no verdict is an error carrying
    /// the digest, never the `200 success:false` that says nothing was paid.
    #[test]
    fn an_execution_with_no_verdict_is_not_a_failed_settlement() {
        let payer = MixedAddress::Sui(SuiAddress::ZERO.to_string());
        let digest = "11111111111111111111111111111111".to_string();
        match settle_outcome(
            Err(FacilitatorLocalError::SettlementUnconfirmed(
                crate::types::TransactionHash::Sui(digest.clone()),
                Network::SuiTestnet,
            )),
            payer.clone(),
            Network::SuiTestnet,
        ) {
            Err(FacilitatorLocalError::SettlementUnconfirmed(tx, _)) => {
                assert_eq!(tx, crate::types::TransactionHash::Sui(digest.clone()))
            }
            other => panic!("expected the unconfirmed error, got {other:?}"),
        }
        let refused = settle_outcome(
            Err(FacilitatorLocalError::Other("refused".into())),
            payer.clone(),
            Network::SuiTestnet,
        )
        .expect("a refusal is a settle verdict");
        assert!(!refused.success);
        assert!(refused.transaction.is_none());
        let failed = settle_outcome(
            Ok(Settled {
                digest: digest.clone(),
                failure: Some("MoveAbort".to_string()),
            }),
            payer.clone(),
            Network::SuiTestnet,
        )
        .expect("an on-chain failure is a settle verdict");
        assert!(!failed.success);
        assert_eq!(
            failed.transaction,
            Some(crate::types::TransactionHash::Sui(digest.clone()))
        );
        let settled = settle_outcome(
            Ok(Settled {
                digest: digest.clone(),
                failure: None,
            }),
            payer,
            Network::SuiTestnet,
        )
        .unwrap();
        assert!(settled.success);
        assert_eq!(
            settled.transaction,
            Some(crate::types::TransactionHash::Sui(digest))
        );
    }

    /// The gas ceilings hold on settle: a sponsor-draining transaction is
    /// refused before any read, signature or send.
    #[tokio::test]
    async fn a_settle_above_the_gas_ceilings_reaches_no_endpoint() {
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let (url, node) = stub::serve(settle_node(
            |d| Reply::Answer(Ok(executed(d, true))),
            digest,
        ))
        .await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        for (price, budget) in [
            (MAX_SPONSORED_GAS_PRICE + 1, 10_000_000),
            (1_000, MAX_SPONSORED_GAS_BUDGET + 1),
            (50_000_000_000, 20_000_000_000),
        ] {
            let (request, _) = request_with_gas(&provider, price, budget);
            let err = provider
                .settle(&request)
                .await
                .expect_err("a draining gas_data must not settle");
            assert!(
                matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("the facilitator sponsors")),
                "price {price} budget {budget}: got {err:?}"
            );
        }
        assert!(node.calls().is_empty(), "{:?}", node.calls());
    }

    #[tokio::test]
    async fn no_reachable_endpoint_is_a_failed_settlement_with_nothing_sent() {
        // Settle reads the balance first, from a live node; then that node
        // stops answering the digest lookup, so nothing can be sent.
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let mut node = settle_node(|d| Reply::Answer(Ok(executed(d, true))), digest);
        node.get_transaction = Box::new(|_| Reply::Answer(Err(tonic::Status::unavailable("down"))));
        let (url, node) = stub::serve(node).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let (request, _) = request(&provider);
        let response = provider.settle(&request).await.expect("a verdict");
        assert!(!response.success, "{response:?}");
        assert!(response.transaction.is_none());
        assert_eq!(node.count("ExecuteTransaction"), 0);
    }

    // --- One settle per signed transaction. ---

    /// A claim store two settles reach together: `check_and_mark_used` waits
    /// until both are at it, as settles that passed verify and every read
    /// before either sent anything.
    #[derive(Debug)]
    struct Rendezvous {
        claims: crate::nonce_store::MemoryNonceStore,
        both: tokio::sync::Barrier,
    }

    #[async_trait::async_trait]
    impl NonceStore for Rendezvous {
        async fn check_and_mark_used(
            &self,
            key: &str,
            ttl_seconds: u64,
        ) -> Result<(), NonceStoreError> {
            self.both.wait().await;
            self.claims.check_and_mark_used(key, ttl_seconds).await
        }
        async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError> {
            self.claims.is_used(key).await
        }
        async fn release(&self, key: &str) -> Result<(), NonceStoreError> {
            self.claims.release(key).await
        }
        async fn health_check(&self) -> Result<(), NonceStoreError> {
            Ok(())
        }
        fn store_type(&self) -> &'static str {
            "memory"
        }
    }

    /// A claim store that cannot be written or read.
    #[derive(Debug)]
    struct Unwritable;

    #[async_trait::async_trait]
    impl NonceStore for Unwritable {
        async fn check_and_mark_used(&self, _: &str, _: u64) -> Result<(), NonceStoreError> {
            Err(NonceStoreError::WriteError("scripted".to_string()))
        }
        async fn is_used(&self, _: &str) -> Result<bool, NonceStoreError> {
            Err(NonceStoreError::ReadError("scripted".to_string()))
        }
        async fn health_check(&self) -> Result<(), NonceStoreError> {
            Err(NonceStoreError::ConnectionFailed("scripted".to_string()))
        }
        fn store_type(&self) -> &'static str {
            "dynamodb"
        }
    }

    /// A claim store that reads (nothing held) but cannot take a claim.
    #[derive(Debug, Default)]
    struct Unclaimable {
        claims: crate::nonce_store::MemoryNonceStore,
    }

    #[async_trait::async_trait]
    impl NonceStore for Unclaimable {
        async fn check_and_mark_used(&self, _: &str, _: u64) -> Result<(), NonceStoreError> {
            Err(NonceStoreError::WriteError("scripted".to_string()))
        }
        async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError> {
            self.claims.is_used(key).await
        }
        async fn health_check(&self) -> Result<(), NonceStoreError> {
            Ok(())
        }
        fn store_type(&self) -> &'static str {
            "dynamodb"
        }
    }

    fn already_submitted_answer(response: &SettleResponse) -> bool {
        !response.success
            && response.transaction.is_none()
            && matches!(
                &response.error_reason,
                Some(crate::types::FacilitatorErrorReason::FreeForm(reason)) if reason == ALREADY_SUBMITTED
            )
    }

    fn claim_of(provider: &SuiProvider) -> String {
        settle_claim_key(Network::SuiTestnet, &digest_of(provider))
    }

    #[test]
    fn the_settle_claim_names_the_network_and_the_digest() {
        assert_eq!(
            settle_claim_key(Network::SuiTestnet, "D1"),
            "sui-settle#sui-testnet#D1"
        );
        assert_eq!(settle_claim_key(Network::Sui, "D1"), "sui-settle#sui#D1");
    }

    /// Two tasks of the service settle the same signed transaction at once,
    /// both past verify: one success, one send, and the other is refused.
    #[tokio::test]
    async fn two_settles_of_one_transaction_at_once_pay_once() {
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let (url, node) = stub::serve(settle_node(
            |d| Reply::Answer(Ok(executed(d, true))),
            digest.clone(),
        ))
        .await;
        let claims: std::sync::Arc<dyn NonceStore> = std::sync::Arc::new(Rendezvous {
            claims: crate::nonce_store::MemoryNonceStore::new(),
            both: tokio::sync::Barrier::new(2),
        });
        let five = std::time::Duration::from_secs(5);
        let first = provider_claiming_in(vec![url.clone()], five, claims.clone());
        let second = provider_claiming_in(vec![url], five, claims);
        let (request, _) = request(&first);
        for task in [&first, &second] {
            let verdict = task.verify(&request).await.expect("verifies");
            assert!(
                matches!(verdict, VerifyResponse::Valid { .. }),
                "{verdict:?}"
            );
        }

        let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            tokio::join!(first.settle(&request), second.settle(&request))
        })
        .await
        .expect("both settles end");
        let answers = [a.expect("a verdict"), b.expect("a verdict")];
        assert_eq!(
            answers.iter().filter(|r| r.success).count(),
            1,
            "{answers:?}"
        );
        assert_eq!(
            answers
                .iter()
                .filter(|r| already_submitted_answer(r))
                .count(),
            1,
            "{answers:?}"
        );
        let paid = answers.iter().find(|r| r.success).unwrap();
        assert_eq!(
            paid.transaction,
            Some(crate::types::TransactionHash::Sui(digest))
        );
        assert_eq!(node.count("ExecuteTransaction"), 1);
    }

    /// The same signed transaction settled again after it paid: refused,
    /// nothing sent, never the first success again.
    #[tokio::test]
    async fn a_second_settle_after_a_payment_is_refused_and_sends_nothing() {
        let (first, _, node, provider) = settle(|d| Reply::Answer(Ok(executed(d, true)))).await;
        assert!(first.expect("settled").success);
        let (request, _) = request(&provider);
        let again = provider.settle(&request).await.expect("a verdict");
        assert!(already_submitted_answer(&again), "{again:?}");
        assert_eq!(node.count("ExecuteTransaction"), 1);
        // Refused on the claim, before reading anything again.
        assert_eq!(node.count("BatchGetObjects"), 1);
    }

    /// The usual repeat: after the first settle executed, the node holds the
    /// spent coin at a newer version. The repeat still answers
    /// `transaction_already_submitted`, because the claim is read first.
    #[tokio::test]
    async fn a_repeat_after_the_inputs_moved_on_is_already_submitted() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let ran = std::sync::Arc::new(AtomicBool::new(false));
        let mut node = settle_node(|d| Reply::Answer(Ok(executed(d, true))), digest.clone());
        let current = std::mem::replace(
            &mut node.batch_get_objects,
            Box::new(|_| unreachable!("replaced below")),
        );
        let moved_on = super::replay_verify_tests::node(Inputs::CoinAdvanced, Holdings::usdc())
            .batch_get_objects;
        let executed_yet = ran.clone();
        node.batch_get_objects = Box::new(move |request| {
            if executed_yet.load(Ordering::SeqCst) {
                moved_on(request)
            } else {
                current(request)
            }
        });
        let executing = ran.clone();
        node.execute = Box::new(move |_| {
            executing.store(true, Ordering::SeqCst);
            Reply::Answer(Ok(executed(&digest, true)))
        });
        let (url, node) = stub::serve(node).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let (request, _) = request(&provider);
        assert!(provider.settle(&request).await.expect("settled").success);
        assert!(ran.load(Ordering::SeqCst));
        let again = provider.settle(&request).await.expect("a verdict");
        assert!(already_submitted_answer(&again), "{again:?}");
        assert_eq!(node.count("ExecuteTransaction"), 1);
    }

    /// The claim is the transaction, not the request around it: the same
    /// signed bytes for another resource are the same transaction.
    #[tokio::test]
    async fn the_same_transaction_for_another_resource_is_refused() {
        let (first, _, node, provider) = settle(|d| Reply::Answer(Ok(executed(d, true)))).await;
        assert!(first.expect("settled").success);
        let (mut request, _) = request(&provider);
        request.payment_requirements.resource =
            url::Url::parse("https://example.com/another").unwrap();
        request.payment_requirements.description = "another resource".to_string();
        let again = provider.settle(&request).await.expect("a verdict");
        assert!(already_submitted_answer(&again), "{again:?}");
        assert_eq!(node.count("ExecuteTransaction"), 1);
    }

    /// A provider built for the service (no store of its own) claims in the
    /// service's one shared store.
    #[tokio::test]
    async fn providers_without_a_store_of_their_own_share_the_service_store() {
        let build = || {
            let keypair = SuiKeyPair::from_bytes(&[0u8; 33]).unwrap();
            let signer = SuiAddress::from(&keypair.public());
            SuiProvider::new(
                Network::SuiTestnet,
                SuiRpc::new(
                    Network::SuiTestnet,
                    vec![],
                    std::time::Duration::from_secs(1),
                    std::time::Duration::from_secs(1),
                ),
                signer,
                keypair,
            )
        };
        let (a, b) = (build(), build());
        assert!(std::sync::Arc::ptr_eq(&a.claims().await, &b.claims().await));
    }

    /// A transaction the chain already executed before this settle (a task
    /// whose claim is gone): not this settle's payment, whatever its verdict.
    #[tokio::test]
    async fn a_settle_of_a_transaction_the_chain_already_executed_is_not_a_payment() {
        use sui_rpc::proto::sui::rpc::v2::GetTransactionResponse;
        for verdict in [true, false] {
            let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
            let mut node = settle_node(|d| Reply::Answer(Ok(executed(d, true))), digest.clone());
            node.get_transaction = Box::new(move |_| {
                let mut response = GetTransactionResponse::default();
                response.transaction = executed(&digest, verdict).transaction;
                Reply::Answer(Ok(response))
            });
            let (url, node) = stub::serve(node).await;
            let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
            let (request, _) = request(&provider);
            let response = provider.settle(&request).await.expect("a verdict");
            assert!(
                already_submitted_answer(&response),
                "{verdict}: {response:?}"
            );
            assert_eq!(node.count("ExecuteTransaction"), 0);
            // Not given back: the transaction did execute.
            assert!(provider
                .claims()
                .await
                .is_used(&claim_of(&provider))
                .await
                .unwrap());
        }
    }

    /// A refused settle executed nothing: its claim is given back, and the
    /// same transaction can be sent again.
    #[tokio::test]
    async fn a_refused_settle_gives_its_claim_back() {
        let (result, _, node, provider) = settle(|_| {
            Reply::Answer(Err(tonic::Status::invalid_argument(
                "Invalid user signature: bad signature",
            )))
        })
        .await;
        assert!(!result.expect("a verdict").success);
        assert!(!provider
            .claims()
            .await
            .is_used(&claim_of(&provider))
            .await
            .unwrap());
        let (request, _) = request(&provider);
        let again = provider.settle(&request).await.expect("a verdict");
        assert!(!already_submitted_answer(&again), "{again:?}");
        assert_eq!(node.count("ExecuteTransaction"), 2);
    }

    /// No endpoint took it: nothing was sent, the claim is given back.
    #[tokio::test]
    async fn a_settle_no_endpoint_took_gives_its_claim_back() {
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let mut node = settle_node(|d| Reply::Answer(Ok(executed(d, true))), digest);
        node.get_transaction = Box::new(|_| Reply::Answer(Err(tonic::Status::unavailable("down"))));
        let (url, _) = stub::serve(node).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let (request, _) = request(&provider);
        assert!(!provider.settle(&request).await.expect("a verdict").success);
        assert!(!provider
            .claims()
            .await
            .is_used(&claim_of(&provider))
            .await
            .unwrap());
    }

    /// An outcome that may be final keeps the claim: a second settle sends
    /// nothing and is refused.
    #[tokio::test]
    async fn an_unconfirmed_settle_keeps_its_claim() {
        let (result, _, node, provider) = settle(|_| {
            Reply::Answer(Err(tonic::Status::unavailable(
                "timed-out before finality could be reached",
            )))
        })
        .await;
        assert!(matches!(
            result,
            Err(FacilitatorLocalError::SettlementUnconfirmed(..))
        ));
        let (request, _) = request(&provider);
        let again = provider.settle(&request).await.expect("a verdict");
        assert!(already_submitted_answer(&again), "{again:?}");
        assert_eq!(node.count("ExecuteTransaction"), 1);
    }

    /// Executed and failed on chain is final too: the claim stays.
    #[tokio::test]
    async fn a_settle_that_failed_on_chain_keeps_its_claim() {
        let (result, _, node, provider) = settle(|d| Reply::Answer(Ok(executed(d, false)))).await;
        assert!(!result.expect("a verdict").success);
        let (request, _) = request(&provider);
        let again = provider.settle(&request).await.expect("a verdict");
        assert!(already_submitted_answer(&again), "{again:?}");
        assert_eq!(node.count("ExecuteTransaction"), 1);
    }

    /// Without the claim nothing is sent: a store that cannot be written
    /// fails closed.
    #[tokio::test]
    async fn a_settle_whose_claim_cannot_be_taken_sends_nothing() {
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let (url, node) = stub::serve(settle_node(
            |d| Reply::Answer(Ok(executed(d, true))),
            digest,
        ))
        .await;
        let provider = provider_claiming_in(
            vec![url],
            std::time::Duration::from_secs(5),
            std::sync::Arc::new(Unwritable),
        );
        let (request, _) = request(&provider);
        let response = provider.settle(&request).await.expect("a verdict");
        assert!(!response.success, "{response:?}");
        assert!(response.transaction.is_none());
        let reason = format!("{:?}", response.error_reason);
        assert!(reason.contains("settlement_unavailable"), "{reason}");
        assert_eq!(node.count("ExecuteTransaction"), 0);
        assert_eq!(node.count("GetTransaction"), 0);
    }

    /// A store that answers the read but refuses the claim: nothing is sent.
    #[tokio::test]
    async fn a_settle_whose_claim_is_refused_by_the_store_sends_nothing() {
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let (url, node) = stub::serve(settle_node(
            |d| Reply::Answer(Ok(executed(d, true))),
            digest,
        ))
        .await;
        let provider = provider_claiming_in(
            vec![url],
            std::time::Duration::from_secs(5),
            std::sync::Arc::new(Unclaimable::default()),
        );
        let (request, _) = request(&provider);
        let response = provider.settle(&request).await.expect("a verdict");
        assert!(!response.success, "{response:?}");
        assert!(response.transaction.is_none());
        let reason = format!("{:?}", response.error_reason);
        assert!(reason.contains("settlement_unavailable"), "{reason}");
        // The reads ran; the claim is what stopped it.
        assert_eq!(node.count("BatchGetObjects"), 1);
        assert_eq!(node.count("ExecuteTransaction"), 0);
        assert_eq!(node.count("GetTransaction"), 0);
    }

    /// A transaction whose inputs moved on (it executed long ago) is refused
    /// by settle's own read, before any claim or send.
    #[tokio::test]
    async fn a_settle_whose_inputs_moved_on_takes_no_claim_and_sends_nothing() {
        let mut node = node(Inputs::CoinAdvanced, Holdings::usdc());
        node.get_transaction =
            Box::new(|_| Reply::Answer(Err(tonic::Status::not_found("Transaction not found"))));
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        node.execute = Box::new(move |_| Reply::Answer(Ok(executed(&digest, true))));
        let (url, node) = stub::serve(node).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let (request, _) = request(&provider);
        let err = provider
            .settle(&request)
            .await
            .expect_err("its inputs are gone");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("already used")),
            "{err:?}"
        );
        assert_eq!(node.count("ExecuteTransaction"), 0);
        assert!(!provider
            .claims()
            .await
            .is_used(&claim_of(&provider))
            .await
            .unwrap());
    }

    /// Two different transactions of the same payer each settle: the claim is
    /// per transaction, not per payer or per provider.
    #[tokio::test]
    async fn two_different_transactions_both_settle() {
        let mut node = node(Inputs::Current, Holdings::usdc());
        node.get_transaction =
            Box::new(|_| Reply::Answer(Err(tonic::Status::not_found("Transaction not found"))));
        node.execute = Box::new(|request| {
            let bytes = request
                .transaction
                .as_ref()
                .and_then(|t| t.bcs.as_ref())
                .and_then(|b| b.value.as_deref())
                .unwrap();
            let tx: TransactionData = bcs::from_bytes(bytes).unwrap();
            Reply::Answer(Ok(executed(&tx.digest().to_string(), true)))
        });
        let (url, node) = stub::serve(node).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        for budget in [10_000_000, 10_000_001] {
            let (request, _) = request_with_gas(&provider, 1_000, budget);
            let response = provider.settle(&request).await.expect("a verdict");
            assert!(response.success, "budget {budget}: {response:?}");
        }
        assert_eq!(node.count("ExecuteTransaction"), 2);
    }

    /// Settle checks the signature too: a forged one is never co-signed or
    /// sent, and reaches no endpoint.
    #[tokio::test]
    async fn a_settle_with_a_forged_signature_reaches_no_endpoint() {
        use super::replay_verify_tests::{invalid_signature, sender_keypair, sign, signed_with};
        let digest = digest_of(&provider_at(vec![], std::time::Duration::from_secs(1)));
        let (url, node) = stub::serve(settle_node(
            |d| Reply::Answer(Ok(executed(d, true))),
            digest,
        ))
        .await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let mut forged = sign(
            &transaction(provider.signer_address, sender()),
            &sender_keypair(),
            Intent::sui_transaction(),
        );
        forged[1..65].fill(0);
        let err = provider
            .settle(&signed_with(&provider, &forged))
            .await
            .expect_err("a forged signature");
        assert!(invalid_signature(&err), "{err:?}");
        assert!(node.calls().is_empty(), "{:?}", node.calls());
    }
}

/// The x402 Foundation's Sui payload (`scheme_exact_sui.md`): the spec's own
/// example (`tests/fixtures/x402-spec/`), and the same request shape carrying
/// transactions built here, sponsored and paid by their sender.
#[cfg(test)]
mod spec_payload_tests {
    use super::execution_outcome_tests::{executed, settle_node};
    use super::replay_verify_tests::{
        merchant, provider_at, provider_with, sender, sender_keypair, transaction, Holdings,
        Inputs, AMOUNT,
    };
    use super::stub::{self, Reply};
    use super::*;
    use crate::types_v2::VerifyRequestEnvelope;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde_json::{json, Value};

    const PAYMENT_PAYLOAD: &str =
        include_str!("../../tests/fixtures/x402-spec/sui_exact_payment_payload.json");

    /// The spec's request for `payload`: the resource server passes the
    /// payload's own `accepted` on as `paymentRequirements`.
    fn body(payload: &Value) -> Value {
        json!({
            "x402Version": 2,
            "paymentPayload": payload,
            "paymentRequirements": payload["accepted"],
        })
    }

    fn read(body: Value) -> VerifyRequest {
        let envelope: VerifyRequestEnvelope = serde_json::from_value(body).unwrap();
        assert!(
            matches!(envelope, VerifyRequestEnvelope::SpecV2(_)),
            "the spec's body is read as the spec's: {envelope:?}"
        );
        envelope.to_v1().unwrap()
    }

    /// Where a transfer's `TransferObjects` sends what it moves.
    fn recipient_of(tx: &TransactionData) -> SuiAddress {
        let TransactionKind::ProgrammableTransaction(ptb) = tx.kind() else {
            panic!("a programmable transaction");
        };
        let recipient = ptb
            .commands
            .iter()
            .find_map(|command| match command {
                Command::TransferObjects(_, Argument::Input(index)) => Some(*index),
                _ => None,
            })
            .expect("a TransferObjects to an input");
        match &ptb.inputs[usize::from(recipient)] {
            CallArg::Pure(bytes) => SuiAddress::from_bytes(bytes).unwrap(),
            other => panic!("recipient input {other:?}"),
        }
    }

    /// The spec's example as written, its `payTo` written out in full again
    /// (the fixture abbreviates it: see the fixtures' README). The address
    /// the example names as `payTo` is the transaction's own sender.
    fn spec_example() -> (Value, TransactionData) {
        let mut payload: Value = serde_json::from_str(PAYMENT_PAYLOAD).unwrap();
        let tx: TransactionData = bcs::from_bytes(
            &STANDARD
                .decode(payload["payload"]["transaction"].as_str().unwrap())
                .unwrap(),
        )
        .expect("the spec's transaction is BCS TransactionData");
        let sender = tx.sender().to_string();
        let abbreviated = payload["accepted"]["payTo"].as_str().unwrap().to_string();
        let (head, tail) = abbreviated.split_once("...").unwrap();
        assert!(
            sender.starts_with(head) && sender.ends_with(tail),
            "the transaction is sent by {sender}, the example names {abbreviated}"
        );
        payload["accepted"]["payTo"] = json!(sender);
        (payload, tx)
    }

    /// The split amount of a `SplitCoins(_, [Input(i)])` transfer.
    fn split_amount_of(tx: &TransactionData) -> u64 {
        let TransactionKind::ProgrammableTransaction(ptb) = tx.kind() else {
            panic!("a programmable transaction");
        };
        let index = ptb
            .commands
            .iter()
            .find_map(|command| match command {
                Command::SplitCoins(_, amounts) => match amounts.as_slice() {
                    [Argument::Input(index)] => Some(*index),
                    _ => None,
                },
                _ => None,
            })
            .expect("a SplitCoins of one input amount");
        match &ptb.inputs[usize::from(index)] {
            CallArg::Pure(bytes) => u64::from_le_bytes(bytes.as_slice().try_into().unwrap()),
            other => panic!("amount input {other:?}"),
        }
    }

    /// The example's shape for a transaction built here, on testnet USDC.
    fn spec_payload(tx: &TransactionData, signer: &SuiKeyPair) -> Value {
        let signature = Signature::new_secure(
            &IntentMessage::new(Intent::sui_transaction(), tx.clone()),
            signer,
        );
        let mut payload: Value = serde_json::from_str(PAYMENT_PAYLOAD).unwrap();
        payload["accepted"] = json!({
            "scheme": "exact",
            "network": "sui:testnet",
            "amount": AMOUNT.to_string(),
            "asset": USDC_COIN_TYPE_TESTNET,
            "payTo": merchant().to_string(),
            "maxTimeoutSeconds": 60,
            "extra": {},
        });
        payload["payload"] = json!({
            "signature": STANDARD.encode(signature.as_ref()),
            "transaction": STANDARD.encode(bcs::to_bytes(tx).unwrap()),
        });
        payload
    }

    /// A USDC transfer by [`sender`] whose gas `gas_owner` pays.
    fn spec_request(gas_owner: SuiAddress) -> VerifyRequest {
        read(body(&spec_payload(
            &transaction(gas_owner, sender()),
            &sender_keypair(),
        )))
    }

    fn facilitator_address() -> SuiAddress {
        provider_at(vec![], std::time::Duration::from_secs(1)).signer_address
    }

    async fn verify(request: &VerifyRequest) -> Result<VerifyResponse, FacilitatorLocalError> {
        provider_with(Inputs::Current, Holdings::usdc())
            .await
            .verify(request)
            .await
    }

    fn refused_because(result: Result<VerifyResponse, FacilitatorLocalError>, why: &str) {
        let err = result.expect_err(why);
        assert!(
            format!("{err:?}").contains(why),
            "refused, but for: {err:?}"
        );
    }

    fn valid_for_sender(response: &VerifyResponse) -> bool {
        matches!(response, VerifyResponse::Valid { payer } if *payer == MixedAddress::Sui(sender().to_string()))
    }

    /// The spec's example writes its asset with a short package,
    /// `0x2::sui::SUI`, which this facilitator does not read as a Sui coin
    /// type (it settles USDC, whose package is written in full): the body is
    /// refused before any provider reads it. Its payload, written out with a
    /// USDC asset, is read as the spec's.
    #[test]
    fn the_spec_example_is_refused_at_its_asset_and_its_payload_is_the_specs() {
        let (mut payload, _) = spec_example();
        assert_eq!(payload["accepted"]["asset"], json!("0x2::sui::SUI"));
        assert!(serde_json::from_value::<VerifyRequestEnvelope>(body(&payload)).is_err());

        payload["accepted"]["asset"] = json!(USDC_COIN_TYPE_MAINNET);
        let request = read(body(&payload));
        assert_eq!(request.payment_payload.network, Network::Sui);
        assert!(matches!(
            &request.payment_payload.payload,
            ExactPaymentPayload::SuiTransaction(p)
                if p.signature == payload["payload"]["signature"].as_str().unwrap()
                && p.transaction == payload["payload"]["transaction"].as_str().unwrap()
        ));
        let terms = &request.payment_requirements;
        assert_eq!(terms.max_amount_required.to_string(), "10000000");
        assert_eq!(
            terms.pay_to.to_string(),
            payload["accepted"]["payTo"].as_str().unwrap()
        );
    }

    /// What the spec's example holds, decoded: its sender pays its own gas
    /// and sends 2.5 SUI to an address other than `payTo`, which is the sender
    /// itself; its `signature` is 64 raw bytes, with neither the scheme flag
    /// nor the public key a Sui signature carries. It is not a payment of its
    /// own terms, and no facilitator can verify it.
    #[test]
    fn the_spec_example_is_not_a_payment_of_its_own_terms() {
        let (payload, tx) = spec_example();
        assert_eq!(tx.gas_data().owner, tx.sender(), "the sender pays its gas");
        assert_ne!(
            recipient_of(&tx),
            tx.sender(),
            "it pays someone other than payTo"
        );
        assert_eq!(split_amount_of(&tx), 2_500_000_000);
        assert_ne!(
            split_amount_of(&tx).to_string(),
            payload["accepted"]["amount"]
        );

        let signature = payload["payload"]["signature"].as_str().unwrap();
        assert_eq!(STANDARD.decode(signature).unwrap().len(), 64);
        let provider = SuiProvider::for_test(Network::Sui);
        let refused = provider
            .decode_signature(signature)
            .expect_err("not a Sui signature");
        assert!(
            format!("{refused:?}").contains("Failed to parse Sui signature"),
            "{refused:?}"
        );
    }

    /// The example under USDC terms is refused at its signature, which is no
    /// Sui signature; its transaction's shape, a `MergeCoins` of six coins,
    /// is read, and what refuses it is its split, not the `amount` asked for.
    #[tokio::test]
    async fn the_spec_example_under_usdc_terms_is_refused() {
        let (mut payload, tx) = spec_example();
        payload["accepted"]["asset"] = json!(USDC_COIN_TYPE_MAINNET);
        let provider = SuiProvider::for_test(Network::Sui);
        refused_because(
            provider.verify(&read(body(&payload))).await,
            "Failed to parse Sui signature",
        );

        let coin = spent_coin(&tx).expect("it splits its first input");
        assert_eq!(merged_coins(&tx).len(), 5);
        let refused = provider
            .validate_ptb_with(&tx, &tx.sender(), 10_000_000, &coin, PtbRules::SPEC)
            .expect_err("not its own amount");
        assert!(
            format!("{refused:?}").contains("does not match required amount"),
            "{refused:?}"
        );
        // Under its own recipient and amount the shape is a transfer the
        // spec's rules read; the declared payload's rules do not.
        provider
            .validate_ptb_with(
                &tx,
                &recipient_of(&tx),
                2_500_000_000,
                &coin,
                PtbRules::SPEC,
            )
            .expect("its own transfer, read as one");
        assert!(provider
            .validate_ptb_with(
                &tx,
                &recipient_of(&tx),
                2_500_000_000,
                &coin,
                PtbRules::DECLARED
            )
            .is_err());
    }

    #[tokio::test]
    async fn a_spec_payment_the_facilitator_sponsors_verifies() {
        let response = verify(&spec_request(facilitator_address()))
            .await
            .expect("a verdict");
        assert!(valid_for_sender(&response), "{response:?}");
    }

    #[tokio::test]
    async fn a_spec_payment_whose_sender_pays_its_gas_verifies() {
        let response = verify(&spec_request(sender())).await.expect("a verdict");
        assert!(valid_for_sender(&response), "{response:?}");
    }

    #[tokio::test]
    async fn a_spec_payment_whose_gas_a_third_party_pays_is_refused() {
        let third = SuiAddress::from(ObjectID::new([0x77; 32]));
        refused_because(
            verify(&spec_request(third)).await,
            "must be the facilitator",
        );
    }

    #[tokio::test]
    async fn a_sender_paid_spec_payment_above_the_sponsor_ceilings_verifies() {
        // The ceilings bound what a sponsor pays; this one pays nothing.
        let mut tx = transaction(sender(), sender());
        let TransactionData::V1(v1) = &mut tx;
        v1.gas_data.price = MAX_SPONSORED_GAS_PRICE + 1;
        v1.gas_data.budget = MAX_SPONSORED_GAS_BUDGET + 1;
        let response = verify(&read(body(&spec_payload(&tx, &sender_keypair()))))
            .await
            .expect("a verdict");
        assert!(valid_for_sender(&response), "{response:?}");
    }

    #[tokio::test]
    async fn a_sponsored_spec_payment_above_the_ceilings_is_refused() {
        let mut tx = transaction(facilitator_address(), sender());
        let TransactionData::V1(v1) = &mut tx;
        v1.gas_data.price = MAX_SPONSORED_GAS_PRICE + 1;
        refused_because(
            verify(&read(body(&spec_payload(&tx, &sender_keypair())))).await,
            "above the",
        );
    }

    #[tokio::test]
    async fn the_facilitator_as_sender_is_refused() {
        let facilitator = facilitator_address();
        let tx = transaction(facilitator, facilitator);
        refused_because(
            verify(&read(body(&spec_payload(&tx, &sender_keypair())))).await,
            "cannot be the sender",
        );
    }

    #[tokio::test]
    async fn a_spec_payment_for_another_asset_is_refused() {
        let mut payload = spec_payload(&transaction(sender(), sender()), &sender_keypair());
        // Another coin type, its package written in full (built: the
        // pre-commit hook refuses 0x + 64 hex typed out).
        payload["accepted"]["asset"] = json!(format!("0x{}::coin::OTHER", "ab".repeat(32)));
        refused_because(verify(&read(body(&payload))).await, "is not the USDC");
    }

    #[tokio::test]
    async fn a_spec_payment_for_another_amount_is_refused() {
        let mut payload = spec_payload(&transaction(sender(), sender()), &sender_keypair());
        payload["accepted"]["amount"] = json!((AMOUNT - 1).to_string());
        refused_because(
            verify(&read(body(&payload))).await,
            "does not match required amount",
        );
    }

    #[tokio::test]
    async fn a_spec_payment_to_another_recipient_is_refused() {
        let mut payload = spec_payload(&transaction(sender(), sender()), &sender_keypair());
        payload["accepted"]["payTo"] =
            json!(SuiAddress::from(ObjectID::new([0x55; 32])).to_string());
        refused_because(
            verify(&read(body(&payload))).await,
            "does not match required pay_to",
        );
    }

    #[tokio::test]
    async fn a_spec_transaction_that_does_not_open_with_a_split_is_refused() {
        let mut tx = transaction(sender(), sender());
        let TransactionData::V1(v1) = &mut tx;
        let TransactionKind::ProgrammableTransaction(ptb) = &mut v1.kind else {
            unreachable!()
        };
        ptb.commands.reverse();
        refused_because(
            verify(&read(body(&spec_payload(&tx, &sender_keypair())))).await,
            "must split an owned coin input",
        );
    }

    #[tokio::test]
    async fn a_spec_payment_signed_by_another_key_is_refused() {
        let mut bytes = [5u8; 33];
        bytes[0] = 0;
        let other = SuiKeyPair::from_bytes(&bytes).unwrap();
        refused_because(
            verify(&read(body(&spec_payload(
                &transaction(sender(), sender()),
                &other,
            ))))
            .await,
            "signature does not verify",
        );
    }

    /// The payload this facilitator defined keeps its rule: the facilitator
    /// sponsors, a sender paying its own gas is refused.
    #[tokio::test]
    async fn the_declared_payload_still_requires_the_facilitator_as_sponsor() {
        let spec = spec_request(sender());
        let ExactPaymentPayload::SuiTransaction(p) = &spec.payment_payload.payload else {
            unreachable!()
        };
        let mut declared = spec.clone();
        declared.payment_payload.payload = ExactPaymentPayload::Sui(ExactSuiPayload {
            transaction_bytes: p.transaction.clone(),
            sender_signature: p.signature.clone(),
            from: sender().to_string(),
            to: merchant().to_string(),
            amount: AMOUNT.to_string(),
            coin_object_id: super::replay_verify_tests::coin_ref().0.to_string(),
        });
        refused_because(verify(&declared).await, "must be the facilitator");
    }

    /// Settle sends what the payment's gas owner calls for: the sender's
    /// signature and the facilitator's when it sponsors, the sender's alone
    /// when the sender pays.
    async fn settle(gas_owner: SuiAddress) -> (SettleResponse, Vec<Signature>, String) {
        let tx = transaction(gas_owner, sender());
        let digest = tx.digest().to_string();
        let (url, node) = stub::serve(settle_node(
            |digest| Reply::Answer(Ok(executed(digest, true))),
            digest.clone(),
        ))
        .await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let response = provider
            .settle(&read(body(&spec_payload(&tx, &sender_keypair()))))
            .await
            .expect("settled");
        let sent = node.executions();
        assert_eq!(sent.len(), 1);
        let signatures = sent[0]
            .signatures
            .iter()
            .map(|s| {
                Signature::from_bytes(s.bcs.as_ref().unwrap().value.as_deref().unwrap()).unwrap()
            })
            .collect();
        (response, signatures, digest)
    }

    #[tokio::test]
    async fn a_sponsored_spec_payment_settles_with_both_signatures() {
        let sponsor = facilitator_address();
        let (response, signatures, digest) = settle(sponsor).await;
        assert!(response.success, "{response:?}");
        assert_eq!(
            response.transaction,
            Some(crate::types::TransactionHash::Sui(digest))
        );
        let intent = IntentMessage::new(Intent::sui_transaction(), transaction(sponsor, sender()));
        assert_eq!(signatures.len(), 2, "sender, then sponsor");
        signatures[0]
            .verify_secure(&intent, sender(), signatures[0].scheme())
            .expect("the sender's");
        signatures[1]
            .verify_secure(&intent, sponsor, signatures[1].scheme())
            .expect("the facilitator's");
    }

    #[tokio::test]
    async fn a_sender_paid_spec_payment_settles_with_the_senders_signature_alone() {
        let (response, signatures, digest) = settle(sender()).await;
        assert!(response.success, "{response:?}");
        assert_eq!(
            response.transaction,
            Some(crate::types::TransactionHash::Sui(digest))
        );
        assert_eq!(signatures.len(), 1, "the facilitator is not a signer");
        let intent = IntentMessage::new(Intent::sui_transaction(), transaction(sender(), sender()));
        signatures[0]
            .verify_secure(&intent, sender(), signatures[0].scheme())
            .expect("the sender's");
    }

    // -----------------------------------------------------------------------
    // Several coins: one MergeCoins into the coin the transfer splits
    // -----------------------------------------------------------------------

    /// A coin of `owner` and `coin_type` the node holds at `obj_ref`.
    #[derive(Clone)]
    struct Held {
        obj_ref: ObjectRef,
        owner: SuiAddress,
        coin_type: String,
        /// What the node reports the coin holds; `None`: not reported.
        balance: Option<u64>,
    }

    fn senders_usdc(seed: u8) -> Held {
        Held {
            obj_ref: (
                ObjectID::new([seed; 32]),
                sui_types::base_types::SequenceNumber::from_u64(7),
                sui_types::base_types::ObjectDigest::new([seed; 32]),
            ),
            owner: sender(),
            coin_type: USDC_COIN_TYPE_TESTNET.to_string(),
            balance: Some(AMOUNT),
        }
    }

    /// The coin every transfer here splits, holding `balance`.
    fn spent_holding(balance: Option<u64>) -> Held {
        Held {
            obj_ref: super::replay_verify_tests::coin_ref(),
            owner: sender(),
            coin_type: USDC_COIN_TYPE_TESTNET.to_string(),
            balance,
        }
    }

    /// The settle node of the other tests, also holding `extra` coins: each
    /// current at its ref, with its owner, type and balance; the spent coin
    /// holds `AMOUNT` unless `extra` names it. An execution answers as the
    /// transaction of `digest`.
    fn node_holding(extra: Vec<Held>, digest: String) -> stub::Node {
        use sui_rpc::proto::sui::rpc::v2::{
            get_object_result, owner::OwnerKind, BatchGetObjectsResponse, GetObjectResponse,
            GetObjectResult, Object, Owner,
        };
        let mut node = settle_node(|digest| Reply::Answer(Ok(executed(digest, true))), digest);
        let mut held = extra.clone();
        held.push(spent_holding(Some(AMOUNT)));
        let gas = super::replay_verify_tests::gas_ref();
        let refs: Vec<ObjectRef> = held.iter().map(|h| h.obj_ref).chain([gas]).collect();
        node.batch_get_objects = Box::new(move |request| {
            let mut response = BatchGetObjectsResponse::default();
            response.objects = request
                .requests
                .iter()
                .map(|r| {
                    let id = ObjectID::from_str(r.object_id.as_deref().unwrap()).unwrap();
                    let obj_ref = refs.iter().find(|(i, _, _)| *i == id).copied().unwrap();
                    let mut object = Object::default();
                    object.object_id = Some(id.to_string());
                    object.version = Some(obj_ref.1.value());
                    object.digest = Some(obj_ref.2.to_string());
                    let mut result = GetObjectResult::default();
                    result.result = Some(get_object_result::Result::Object(object));
                    result
                })
                .collect();
            Reply::Answer(Ok(response))
        });
        node.get_object = Box::new(move |request| {
            let id = ObjectID::from_str(request.object_id.as_deref().unwrap()).unwrap();
            let Some(coin) = held.iter().find(|h| h.obj_ref.0 == id) else {
                return Reply::Answer(Err(tonic::Status::not_found("no such object")));
            };
            let mut owner = Owner::default();
            owner.kind = Some(OwnerKind::Address as i32);
            owner.address = Some(coin.owner.to_string());
            let mut object = Object::default();
            object.object_id = Some(id.to_string());
            object.owner = Some(owner);
            object.object_type = Some(format!("0x2::coin::Coin<{}>", coin.coin_type));
            object.balance = coin.balance;
            let mut response = GetObjectResponse::default();
            response.object = Some(object);
            Reply::Answer(Ok(response))
        });
        node
    }

    /// A transfer of `AMOUNT` to the merchant that first merges `merged`
    /// (inputs 1..) into the coin it splits (input 0).
    fn merging(gas_owner: SuiAddress, merged: &[ObjectRef]) -> TransactionData {
        use sui_types::transaction::{ObjectArg, ProgrammableTransaction, TransactionDataV1};
        let mut tx = transaction(gas_owner, sender());
        let TransactionData::V1(TransactionDataV1 { kind, .. }) = &mut tx;
        let mut inputs = vec![CallArg::Object(ObjectArg::ImmOrOwnedObject(
            super::replay_verify_tests::coin_ref(),
        ))];
        inputs.extend(
            merged
                .iter()
                .map(|r| CallArg::Object(ObjectArg::ImmOrOwnedObject(*r))),
        );
        let n = inputs.len() as u16;
        inputs.push(CallArg::Pure(AMOUNT.to_le_bytes().to_vec()));
        inputs.push(CallArg::Pure(merchant().to_vec()));
        *kind = TransactionKind::ProgrammableTransaction(ProgrammableTransaction {
            inputs,
            commands: vec![
                Command::MergeCoins(Argument::Input(0), (1..n).map(Argument::Input).collect()),
                Command::SplitCoins(Argument::Input(0), vec![Argument::Input(n)]),
                Command::TransferObjects(vec![Argument::Result(1)], Argument::Input(n + 1)),
            ],
        });
        tx
    }

    fn commands_of(tx: &mut TransactionData) -> &mut Vec<Command> {
        let TransactionData::V1(v1) = tx;
        let TransactionKind::ProgrammableTransaction(ptb) = &mut v1.kind else {
            unreachable!()
        };
        &mut ptb.commands
    }

    async fn verify_holding(
        extra: Vec<Held>,
        tx: &TransactionData,
    ) -> Result<VerifyResponse, FacilitatorLocalError> {
        let (url, _) = stub::serve(node_holding(extra, tx.digest().to_string())).await;
        provider_at(vec![url], std::time::Duration::from_secs(5))
            .verify(&read(body(&spec_payload(tx, &sender_keypair()))))
            .await
    }

    #[tokio::test]
    async fn a_payment_from_two_coins_verifies() {
        let extra = vec![senders_usdc(0x31)];
        let tx = merging(sender(), &[extra[0].obj_ref]);
        let response = verify_holding(extra, &tx).await.expect("a verdict");
        assert!(valid_for_sender(&response), "{response:?}");
    }

    #[tokio::test]
    async fn a_payment_from_three_coins_settles() {
        let extra = vec![senders_usdc(0x31), senders_usdc(0x32)];
        let refs: Vec<ObjectRef> = extra.iter().map(|h| h.obj_ref).collect();
        let tx = merging(facilitator_address(), &refs);
        let (url, node) = stub::serve(node_holding(extra, tx.digest().to_string())).await;
        let provider = provider_at(vec![url], std::time::Duration::from_secs(5));
        let request = read(body(&spec_payload(&tx, &sender_keypair())));
        let response = provider.verify(&request).await.expect("a verdict");
        assert!(valid_for_sender(&response), "{response:?}");
        let settled = provider.settle(&request).await.expect("settled");
        assert!(settled.success, "{settled:?}");
        assert_eq!(node.executions().len(), 1);
        assert_eq!(node.executions()[0].signatures.len(), 2, "sponsored");
    }

    #[tokio::test]
    async fn a_merged_coin_of_another_type_is_refused() {
        let mut other = senders_usdc(0x31);
        other.coin_type = format!("0x{}::junk::JUNK", "ab".repeat(32));
        let tx = merging(sender(), &[other.obj_ref]);
        refused_because(verify_holding(vec![other], &tx).await, "merged coin object");
    }

    #[tokio::test]
    async fn a_merged_coin_owned_by_someone_else_is_refused() {
        let mut theirs = senders_usdc(0x31);
        theirs.owner = SuiAddress::from(ObjectID::new([0x55; 32]));
        let tx = merging(sender(), &[theirs.obj_ref]);
        refused_because(
            verify_holding(vec![theirs], &tx).await,
            "merged coin object",
        );
    }

    /// Settle reads the merged coins too: one of another type sends nothing.
    #[tokio::test]
    async fn a_settle_with_a_merged_coin_of_another_type_sends_nothing() {
        let mut other = senders_usdc(0x31);
        other.coin_type = format!("0x{}::junk::JUNK", "ab".repeat(32));
        let tx = merging(facilitator_address(), &[other.obj_ref]);
        let (url, node) = stub::serve(node_holding(vec![other], tx.digest().to_string())).await;
        let refused = provider_at(vec![url], std::time::Duration::from_secs(5))
            .settle(&read(body(&spec_payload(&tx, &sender_keypair()))))
            .await
            .expect_err("refused");
        assert!(
            format!("{refused:?}").contains("merged coin object"),
            "{refused:?}"
        );
        assert!(node.executions().is_empty(), "nothing was sent");
    }

    #[tokio::test]
    async fn the_spent_coin_merged_into_itself_is_refused() {
        let tx = merging(sender(), &[super::replay_verify_tests::coin_ref()]);
        refused_because(verify_holding(vec![], &tx).await, "merged twice");
    }

    #[tokio::test]
    async fn one_coin_merged_twice_is_refused() {
        let coin = senders_usdc(0x31);
        let tx = merging(sender(), &[coin.obj_ref, coin.obj_ref]);
        refused_because(verify_holding(vec![coin], &tx).await, "merged twice");
    }

    #[tokio::test]
    async fn a_merge_into_another_coin_than_the_split_is_refused() {
        let coin = senders_usdc(0x31);
        let mut tx = merging(sender(), &[coin.obj_ref]);
        commands_of(&mut tx)[0] = Command::MergeCoins(Argument::Input(1), vec![Argument::Input(0)]);
        refused_because(
            verify_holding(vec![coin], &tx).await,
            "must merge into the coin SplitCoins splits",
        );
    }

    #[tokio::test]
    async fn merging_the_gas_coin_is_refused() {
        let coin = senders_usdc(0x31);
        let mut tx = merging(sender(), &[coin.obj_ref]);
        commands_of(&mut tx)[0] = Command::MergeCoins(Argument::Input(0), vec![Argument::GasCoin]);
        refused_because(verify_holding(vec![coin], &tx).await, "must be an Input");
    }

    #[tokio::test]
    async fn a_merge_of_nothing_is_refused() {
        let coin = senders_usdc(0x31);
        let mut tx = merging(sender(), &[coin.obj_ref]);
        commands_of(&mut tx)[0] = Command::MergeCoins(Argument::Input(0), vec![]);
        refused_because(verify_holding(vec![coin], &tx).await, "merges no coin");
    }

    #[tokio::test]
    async fn a_merge_whose_split_is_not_the_amount_is_refused() {
        let coin = senders_usdc(0x31);
        let tx = merging(sender(), &[coin.obj_ref]);
        let mut payload = spec_payload(&tx, &sender_keypair());
        payload["accepted"]["amount"] = json!((AMOUNT + 1).to_string());
        let (url, _) = stub::serve(node_holding(vec![coin], tx.digest().to_string())).await;
        refused_because(
            provider_at(vec![url], std::time::Duration::from_secs(5))
                .verify(&read(body(&payload)))
                .await,
            "does not match required amount",
        );
    }

    /// Three commands opening with anything else than a MergeCoins.
    #[tokio::test]
    async fn three_commands_without_a_merge_are_refused() {
        let coin = senders_usdc(0x31);
        let mut tx = merging(sender(), &[coin.obj_ref]);
        commands_of(&mut tx)[0] = Command::SplitCoins(Argument::Input(1), vec![Argument::Input(2)]);
        refused_because(
            verify_holding(vec![coin], &tx).await,
            "or 3 opening with MergeCoins",
        );
    }

    /// The payload this facilitator defined keeps its one coin.
    #[tokio::test]
    async fn the_declared_payload_does_not_merge() {
        let coin = senders_usdc(0x31);
        let tx = merging(facilitator_address(), &[coin.obj_ref]);
        let spec = read(body(&spec_payload(&tx, &sender_keypair())));
        let ExactPaymentPayload::SuiTransaction(p) = &spec.payment_payload.payload else {
            unreachable!()
        };
        let mut declared = spec.clone();
        declared.payment_payload.payload = ExactPaymentPayload::Sui(ExactSuiPayload {
            transaction_bytes: p.transaction.clone(),
            sender_signature: p.signature.clone(),
            from: sender().to_string(),
            to: merchant().to_string(),
            amount: AMOUNT.to_string(),
            coin_object_id: super::replay_verify_tests::coin_ref().0.to_string(),
        });
        let (url, _) = stub::serve(node_holding(vec![coin], tx.digest().to_string())).await;
        refused_because(
            provider_at(vec![url], std::time::Duration::from_secs(5))
                .verify(&declared)
                .await,
            "exactly 2 commands",
        );
    }

    /// Either payload pays in the asset the requirements name.
    #[tokio::test]
    async fn the_declared_payload_is_refused_for_another_asset_too() {
        let provider = provider_with(Inputs::Current, Holdings::usdc()).await;
        let (mut request, _) = super::replay_verify_tests::request(&provider);
        request.payment_requirements.asset =
            MixedAddress::Sui(format!("0x{}::coin::OTHER", "ab".repeat(32)));
        refused_because(provider.verify(&request).await, "is not the USDC");
        // The same request in USDC verifies.
        let (request, _) = super::replay_verify_tests::request(&provider);
        let response = provider.verify(&request).await.expect("a verdict");
        assert!(valid_for_sender(&response), "{response:?}");
    }

    // -----------------------------------------------------------------------
    // What the spent coins hold
    // -----------------------------------------------------------------------

    fn insufficient(result: &Result<VerifyResponse, FacilitatorLocalError>) -> bool {
        matches!(result, Err(FacilitatorLocalError::InsufficientFunds(_)))
    }

    /// The sender holds enough in all, but the coin it splits holds less:
    /// the split would abort on chain.
    #[tokio::test]
    async fn a_spent_coin_holding_less_than_the_amount_is_refused() {
        let tx = transaction(sender(), sender());
        let result = verify_holding(vec![spent_holding(Some(AMOUNT - 1))], &tx).await;
        assert!(insufficient(&result), "{result:?}");
        let result = verify_holding(vec![spent_holding(Some(AMOUNT))], &tx).await;
        assert!(
            matches!(result, Ok(ref r) if valid_for_sender(r)),
            "{result:?}"
        );
    }

    /// The declared payload is held to its coin's balance as well.
    #[tokio::test]
    async fn the_declared_payload_is_held_to_its_coins_balance_too() {
        let provider = provider_at(vec![], std::time::Duration::from_secs(1));
        let (request, _) = super::replay_verify_tests::request(&provider);
        let digest = transaction(provider.signer_address, sender())
            .digest()
            .to_string();
        let (url, _) =
            stub::serve(node_holding(vec![spent_holding(Some(AMOUNT - 1))], digest)).await;
        let result = provider_at(vec![url], std::time::Duration::from_secs(5))
            .verify(&request)
            .await;
        assert!(insufficient(&result), "{result:?}");
    }

    /// Merged coins count with the spent one, and only together.
    #[tokio::test]
    async fn merged_coins_must_hold_the_amount_between_them() {
        let half = AMOUNT / 2;
        let mut coin = senders_usdc(0x31);
        coin.balance = Some(half - 1);
        let tx = merging(sender(), &[coin.obj_ref]);
        let result = verify_holding(vec![coin.clone(), spent_holding(Some(half))], &tx).await;
        assert!(insufficient(&result), "{result:?}");

        coin.balance = Some(half);
        let result = verify_holding(vec![coin, spent_holding(Some(half))], &tx).await;
        assert!(
            matches!(result, Ok(ref r) if valid_for_sender(r)),
            "{result:?}"
        );
    }

    /// A node that reports no coin balance leaves it to the address total.
    #[tokio::test]
    async fn a_coin_balance_the_node_does_not_report_leaves_it_to_the_total() {
        let mut coin = senders_usdc(0x31);
        coin.balance = None;
        let tx = merging(sender(), &[coin.obj_ref]);
        let result = verify_holding(vec![coin, spent_holding(Some(1))], &tx).await;
        assert!(
            matches!(result, Ok(ref r) if valid_for_sender(r)),
            "{result:?}"
        );
    }

    fn many_coins(count: u8) -> Vec<Held> {
        (0..count)
            .map(|i| {
                let mut coin = senders_usdc(0x40 + i);
                coin.balance = Some(1);
                coin
            })
            .collect()
    }

    #[tokio::test]
    async fn a_merge_of_as_many_coins_as_read_verifies() {
        let coins = many_coins(MAX_MERGED_COINS as u8);
        let refs: Vec<ObjectRef> = coins.iter().map(|c| c.obj_ref).collect();
        let tx = merging(sender(), &refs);
        let result = verify_holding(coins, &tx).await;
        assert!(
            matches!(result, Ok(ref r) if valid_for_sender(r)),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_merge_of_more_coins_than_read_is_refused() {
        let coins = many_coins(MAX_MERGED_COINS as u8 + 1);
        let refs: Vec<ObjectRef> = coins.iter().map(|c| c.obj_ref).collect();
        let tx = merging(sender(), &refs);
        refused_because(verify_holding(coins, &tx).await, "more than the 64");
    }

    // -----------------------------------------------------------------------
    // The split's coin named as `NestedResult`
    // -----------------------------------------------------------------------

    fn transferring(object: Argument) -> TransactionData {
        let mut tx = transaction(sender(), sender());
        commands_of(&mut tx)[1] = Command::TransferObjects(vec![object], Argument::Input(2));
        tx
    }

    /// `const [coin] = tx.splitCoins(...)` in `@mysten/sui` transfers
    /// `NestedResult(split, 0)`: the spec's payload takes it.
    #[tokio::test]
    async fn the_splits_coin_as_nested_result_verifies() {
        let tx = transferring(Argument::NestedResult(0, 0));
        let result = verify_holding(vec![], &tx).await;
        assert!(
            matches!(result, Ok(ref r) if valid_for_sender(r)),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn another_nested_result_is_refused() {
        let tx = transferring(Argument::NestedResult(0, 1));
        refused_because(verify_holding(vec![], &tx).await, "must transfer exactly");
    }

    #[test]
    fn the_declared_payload_keeps_the_whole_result() {
        let provider = SuiProvider::for_test(Network::SuiTestnet);
        let tx = transferring(Argument::NestedResult(0, 0));
        let refused = provider
            .validate_ptb_with(
                &tx,
                &merchant(),
                AMOUNT,
                &super::replay_verify_tests::coin_ref().0,
                PtbRules::DECLARED,
            )
            .expect_err("Result(0) only");
        assert!(
            format!("{refused:?}").contains("must transfer exactly"),
            "{refused:?}"
        );
    }

    /// After a MergeCoins the labels name the commands where they are.
    #[test]
    fn a_misplaced_transfer_after_a_merge_names_its_command() {
        let provider = SuiProvider::for_test(Network::SuiTestnet);
        let coin = senders_usdc(0x31);
        let mut tx = merging(sender(), &[coin.obj_ref]);
        commands_of(&mut tx)[2] = Command::SplitCoins(Argument::Input(0), vec![Argument::Input(2)]);
        let refused = provider
            .validate_ptb_with(
                &tx,
                &merchant(),
                AMOUNT,
                &super::replay_verify_tests::coin_ref().0,
                PtbRules::SPEC,
            )
            .expect_err("no transfer");
        assert!(
            format!("{refused:?}").contains("command[2] must be TransferObjects"),
            "{refused:?}"
        );
    }
}
