//! Inspect the exact bytes the payer signed, including EVERY node variant.
use super::id::EntityId;
use base64::{engine::general_purpose::STANDARD, Engine};
use hiero_sdk::{AnyTransaction, PrivateKey, PublicKey, TransferTransaction};
use hiero_sdk_proto::{sdk::TransactionList, services as pb};
use prost::Message;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_BYTES: usize = 131_072;
pub const MAX_VARIANTS: usize = 32;
pub const MAX_SIGNATURES: usize = 32;
macro_rules! require {
    ($v:expr, $e:expr) => {
        if !$v {
            return Err($e.into());
        }
    };
}

#[derive(Clone)]
pub struct Variant {
    pub body: pb::TransactionBody,
    pub signed: pb::SignedTransaction,
}
#[derive(Clone)]
pub struct Decoded {
    pub bytes: Vec<u8>,
    pub variants: Vec<Variant>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Intent {
    pub transaction_id: String,
    pub fingerprint: String,
    pub payer: EntityId,
    pub pay_to: EntityId,
    pub asset: EntityId,
    pub amount: i64,
    pub fee: u64,
    pub expires_at: u64,
    pub expected_decimals: Option<u32>,
}
pub struct Policy<'a> {
    pub network: &'a str,
    pub fee_payer: &'a EntityId,
    pub pay_to: &'a EntityId,
    pub asset: &'a EntityId,
    pub amount: i64,
    pub max_fee: u64,
    pub max_duration: u64,
    pub allowed_nodes: &'a BTreeSet<EntityId>,
}

/// prost discards unknown fields. Inspect the wire before decoding so neither
/// unknown operations nor duplicate singular/oneof fields can hide behind it.
/// Explicit default fields and field order emitted by protobuf.js are valid;
/// requiring encode(decode(bytes)) == bytes would reject the official client.
fn strict_decode<M: Message + Default>(bytes: &[u8]) -> Result<M> {
    let value = M::decode(bytes).map_err(|_| "invalid Hedera protobuf")?;
    Ok(value)
}
#[derive(Clone, Copy)]
enum Wire {
    List,
    Transaction,
    Signed,
    Body,
    Id,
    Account,
    Token,
    Timestamp,
    Duration,
    Transfer,
    Legs,
    Leg,
    TokenLegs,
    SigMap,
    SigPair,
    U32,
}
fn varint(bytes: &[u8], position: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*position).ok_or("truncated protobuf varint")?;
        *position += 1;
        require!(shift != 63 || byte <= 1, "protobuf varint overflow");
        value |= ((byte & 127) as u64) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err("invalid protobuf varint".into())
}
fn inspect_wire(bytes: &[u8], schema: Wire, depth: usize) -> Result<()> {
    require!(depth <= 16, "protobuf nesting too deep");
    let mut position = 0;
    let mut seen = BTreeSet::new();
    let mut repeated = 0;
    while position < bytes.len() {
        let tag = varint(bytes, &mut position)?;
        let field = tag >> 3;
        let kind = tag & 7;
        use Wire::*;
        let (expected_kind, child, repeats) = match (schema, field) {
            (List, 1) => (2, Some(Transaction), true),
            (Transaction, 5) => (2, Some(Signed), false),
            (Signed, 1) => (2, Some(Body), false),
            (Signed, 2) => (2, Some(SigMap), false),
            (Signed, 3) => (0, None, false),
            (Body, 1) => (2, Some(Id), false),
            (Body, 2) => (2, Some(Account), false),
            (Body, 3 | 5) => (0, None, false),
            (Body, 4) => (2, Some(Duration), false),
            (Body, 6) => (2, None, false),
            (Body, 14) => (2, Some(Transfer), false),
            (Id, 1) => (2, Some(Timestamp), false),
            (Id, 2) => (2, Some(Account), false),
            (Id, 3 | 4)
            | (Account, 1..=3)
            | (Token, 1..=3)
            | (Timestamp, 1 | 2)
            | (Duration, 1)
            | (U32, 1) => (0, None, false),
            (Transfer, 1) => (2, Some(Legs), false),
            (Transfer, 2) => (2, Some(TokenLegs), true),
            (Legs, 1) => (2, Some(Leg), true),
            (Leg, 1) => (2, Some(Account), false),
            (Leg, 2 | 3) => (0, None, false),
            (TokenLegs, 1) => (2, Some(Token), false),
            (TokenLegs, 2) => (2, Some(Leg), true),
            (TokenLegs, 4) => (2, Some(U32), false),
            (SigMap, 1) => (2, Some(SigPair), true),
            (SigPair, 1 | 3 | 6) => (2, None, false),
            _ => return Err("unsupported Hedera protobuf field or operation".into()),
        };
        require!(kind == expected_kind, "incorrect protobuf wire type");
        let slot = if matches!(schema, SigPair) && field != 1 {
            3
        } else {
            field
        };
        require!(
            repeats || seen.insert(slot),
            "duplicate singular or oneof protobuf field"
        );
        if repeats {
            repeated += 1;
            require!(
                repeated <= MAX_VARIANTS.max(MAX_SIGNATURES + 1),
                "too many repeated protobuf fields"
            );
        }
        if kind == 0 {
            varint(bytes, &mut position)?;
        } else {
            let len = usize::try_from(varint(bytes, &mut position)?)
                .map_err(|_| "protobuf length overflow")?;
            let end = position
                .checked_add(len)
                .filter(|end| *end <= bytes.len())
                .ok_or("truncated protobuf field")?;
            if let Some(child) = child {
                inspect_wire(&bytes[position..end], child, depth + 1)?;
            }
            position = end;
        }
    }
    Ok(())
}
impl Decoded {
    pub fn screening_parties(&self) -> Result<(EntityId, EntityId, i64, String)> {
        let Some(pb::transaction_body::Data::CryptoTransfer(t)) =
            &self.variants.first().ok_or("empty variants")?.body.data
        else {
            return Err("not a transfer".into());
        };
        let (legs, currency) = if t.token_transfers.is_empty() {
            (
                t.transfers
                    .as_ref()
                    .ok_or("missing transfers")?
                    .account_amounts
                    .as_slice(),
                "HBAR".to_owned(),
            )
        } else if t.token_transfers.len() == 1 {
            (
                t.token_transfers[0].transfers.as_slice(),
                token_id(t.token_transfers[0].token.as_ref())?.to_string(),
            )
        } else {
            return Err("multiple assets unsupported".into());
        };
        require!(legs.len() == 2, "ambiguous transfer parties");
        let debit = legs.iter().find(|t| t.amount < 0).ok_or("missing debit")?;
        let credit = legs.iter().find(|t| t.amount > 0).ok_or("missing credit")?;
        Ok((
            account(debit.account_id.as_ref())?,
            account(credit.account_id.as_ref())?,
            credit.amount,
            currency,
        ))
    }
    pub fn from_base64(encoded: &str) -> Result<Self> {
        require!(
            !encoded.is_empty() && encoded.len() <= MAX_BYTES.div_ceil(3) * 4,
            "Hedera payload too large or empty"
        );
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "invalid Hedera base64")?;
        Self::from_bytes(bytes)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        require!(bytes.len() <= MAX_BYTES, "Hedera payload too large");
        inspect_wire(&bytes, Wire::List, 0)?;
        let list: TransactionList = strict_decode(&bytes)?;
        require!(
            !list.transaction_list.is_empty() && list.transaction_list.len() <= MAX_VARIANTS,
            "invalid node variant count"
        );
        let mut variants = Vec::new();
        for entry in list.transaction_list {
            #[allow(deprecated)]
            {
                require!(
                    entry.body.is_none()
                        && entry.sigs.is_none()
                        && entry.sig_map.is_none()
                        && entry.body_bytes.is_empty(),
                    "legacy transaction container unsupported"
                );
            }
            let signed: pb::SignedTransaction = strict_decode(&entry.signed_transaction_bytes)?;
            require!(
                !signed.use_serialized_tx_message_hash_algorithm,
                "unsupported signature hash mode"
            );
            let body: pb::TransactionBody = strict_decode(&signed.body_bytes)?;
            let pairs = &signed
                .sig_map
                .as_ref()
                .ok_or("missing signatures")?
                .sig_pair;
            require!(
                !pairs.is_empty() && pairs.len() <= MAX_SIGNATURES,
                "invalid signature count"
            );
            let mut keys = BTreeSet::new();
            for sig in pairs {
                let valid = match &sig.signature {
                    Some(pb::signature_pair::Signature::Ed25519(s)) => {
                        sig.pub_key_prefix.len() == 32 && s.len() == 64
                    }
                    Some(pb::signature_pair::Signature::EcdsaSecp256k1(s)) => {
                        sig.pub_key_prefix.len() == 33 && s.len() == 64
                    }
                    _ => false,
                };
                require!(
                    valid && keys.insert(sig.pub_key_prefix.clone()),
                    "invalid, ambiguous or duplicate signature key"
                );
            }
            variants.push(Variant { body, signed });
        }
        Ok(Self { bytes, variants })
    }
    pub fn inspect(&self, policy: &Policy<'_>, now: u64, check_time: bool) -> Result<Intent> {
        require!(policy.amount > 0, "amount must be positive and fit i64");
        require!(!policy.pay_to.is_hbar(), "invalid recipient");
        require!(
            self.variants.iter().all(|v| v
                .signed
                .sig_map
                .as_ref()
                .is_some_and(|m| m.sig_pair.len() < MAX_SIGNATURES)),
            "too many client signatures to add sponsor"
        );
        let first = &self.variants.first().ok_or("empty variants")?.body;
        let mut normalized = first.clone();
        normalized.node_account_id = None;
        let id = first
            .transaction_id
            .as_ref()
            .ok_or("missing transaction ID")?;
        require!(
            !id.scheduled && id.nonce == 0,
            "scheduled or child transaction unsupported"
        );
        require!(
            &account(id.account_id.as_ref())? == policy.fee_payer,
            "fee payer does not match configured signer"
        );
        let start = id
            .transaction_valid_start
            .as_ref()
            .ok_or("missing valid start")?;
        require!(
            start.seconds > 0 && (0..1_000_000_000).contains(&start.nanos),
            "invalid valid start"
        );
        let duration = first
            .transaction_valid_duration
            .as_ref()
            .ok_or("missing duration")?
            .seconds;
        require!(
            (15..=180).contains(&duration) && duration as u64 <= policy.max_duration,
            "transaction duration exceeds requirements"
        );
        let expiry = (start.seconds as u64)
            .checked_add(duration as u64)
            .ok_or("timestamp overflow")?;
        if check_time {
            require!(
                start.seconds as u64 <= now.saturating_add(5) && expiry >= now.saturating_add(5),
                "transaction expired or starts in the future"
            );
        }
        require!(
            first.transaction_fee > 0 && first.transaction_fee <= policy.max_fee,
            "transaction fee exceeds sponsor limit"
        );
        let mut nodes = BTreeSet::new();
        let first_shape: BTreeSet<_> = self.variants[0]
            .signed
            .sig_map
            .as_ref()
            .unwrap()
            .sig_pair
            .iter()
            .map(|s| s.pub_key_prefix.clone())
            .collect();
        for variant in &self.variants {
            let body = &variant.body;
            let node = account(body.node_account_id.as_ref())?;
            require!(
                policy.allowed_nodes.contains(&node) && nodes.insert(node),
                "unknown or duplicate consensus node"
            );
            let mut comparable = body.clone();
            comparable.node_account_id = None;
            require!(
                comparable == normalized,
                "node variants disagree about payment intent"
            );
            #[allow(deprecated)]
            {
                require!(!body.generate_record, "generateRecord unsupported");
            }
            require!(
                body.batch_key.is_none()
                    && body.max_custom_fees.is_empty()
                    && !body.high_volume
                    && body.memo.len() <= 100,
                "batch, custom fees, high volume or oversized memo unsupported"
            );
            let shape: BTreeSet<_> = variant
                .signed
                .sig_map
                .as_ref()
                .unwrap()
                .sig_pair
                .iter()
                .map(|s| s.pub_key_prefix.clone())
                .collect();
            require!(
                shape == first_shape,
                "node variants have different signer sets"
            );
        }
        let Some(pb::transaction_body::Data::CryptoTransfer(transfer)) = &first.data else {
            return Err("only native CryptoTransfer is supported".into());
        };
        let hbar = transfer
            .transfers
            .as_ref()
            .map(|t| t.account_amounts.as_slice())
            .unwrap_or_default();
        let (legs, decimals) = if policy.asset.is_hbar() {
            require!(
                transfer.token_transfers.is_empty(),
                "HBAR payment includes token transfers"
            );
            (hbar, Some(8))
        } else {
            require!(
                hbar.is_empty() && transfer.token_transfers.len() == 1,
                "HTS payment includes other assets"
            );
            let token = &transfer.token_transfers[0];
            require!(
                &token_id(token.token.as_ref())? == policy.asset && token.nft_transfers.is_empty(),
                "wrong token or NFT rider"
            );
            (token.transfers.as_slice(), token.expected_decimals)
        };
        require!(
            legs.len() == 2,
            "payment must have exactly one sender and recipient"
        );
        let mut payer = None;
        let mut payee = None;
        for leg in legs {
            require!(
                !leg.is_approval && leg.hook_call.is_none(),
                "allowances and hooks unsupported"
            );
            let who = account(leg.account_id.as_ref())?;
            require!(
                leg.amount >= 0 || &who != policy.fee_payer,
                "sponsor principal must never be debited"
            );
            if leg.amount == -policy.amount {
                require!(payer.replace(who).is_none(), "duplicate sender");
            } else if leg.amount == policy.amount {
                require!(payee.replace(who).is_none(), "duplicate recipient");
            } else {
                return Err("payment amount mismatch".into());
            }
        }
        let payer = payer.ok_or("missing payer")?;
        require!(
            payee.as_ref() == Some(policy.pay_to) && &payer != policy.pay_to,
            "recipient mismatch or self payment"
        );
        let transaction_id = format!("{}@{}.{:09}", policy.fee_payer, start.seconds, start.nanos);
        let mut hash = Sha256::new();
        hash.update(policy.network.as_bytes());
        hash.update([0]);
        hash.update(normalized.encode_to_vec());
        Ok(Intent {
            transaction_id,
            fingerprint: hex::encode(hash.finalize()),
            payer,
            pay_to: policy.pay_to.clone(),
            asset: policy.asset.clone(),
            amount: policy.amount,
            fee: first.transaction_fee,
            expires_at: expiry,
            expected_decimals: decimals,
        })
    }
    pub fn verify_key(&self, key: &pb::Key) -> Result<()> {
        let mut leaves = BTreeSet::new();
        validate_key(key, 0, &mut leaves)?;
        for variant in &self.variants {
            require!(
                satisfies(key, variant)?,
                "payer signature does not satisfy account key"
            );
        }
        Ok(())
    }
    pub fn cosign(&self, key: &PrivateKey) -> Result<Vec<u8>> {
        let public = key.public_key();
        let prefix = public.to_bytes_raw();
        require!(
            self.variants.iter().all(|v| !v
                .signed
                .sig_map
                .as_ref()
                .unwrap()
                .sig_pair
                .iter()
                .any(|p| p.pub_key_prefix == prefix)),
            "payload already contains sponsor signature"
        );
        let mut tx: TransferTransaction = AnyTransaction::from_bytes(&self.bytes)
            .map_err(|_| "SDK rejected transaction")?
            .downcast()
            .map_err(|_| "SDK rejected transfer type")?;
        tx.sign(key.clone());
        let bytes = tx
            .to_bytes()
            .map_err(|_| "failed to serialize co-signed transaction")?;
        let after = Self::from_bytes(bytes.clone())?;
        require!(
            after.variants.len() == self.variants.len(),
            "co-sign changed variant count"
        );
        for (before, after) in self.variants.iter().zip(&after.variants) {
            require!(
                before.signed.body_bytes == after.signed.body_bytes,
                "co-sign changed frozen body"
            );
            let old = &before.signed.sig_map.as_ref().unwrap().sig_pair;
            let new = &after.signed.sig_map.as_ref().unwrap().sig_pair;
            require!(
                new.len() == old.len() + 1 && old.iter().all(|p| new.contains(p)),
                "co-sign changed payer signatures"
            );
            let added = new
                .iter()
                .find(|p| p.pub_key_prefix == prefix)
                .ok_or("missing sponsor signature")?;
            require!(
                public
                    .verify(&after.signed.body_bytes, signature_bytes(added)?)
                    .is_ok(),
                "invalid sponsor signature"
            );
        }
        Ok(bytes)
    }
}

pub fn account(id: Option<&pb::AccountId>) -> Result<EntityId> {
    let id = id.ok_or("missing account ID")?;
    let Some(pb::account_id::Account::AccountNum(n)) = id.account else {
        return Err("account aliases unsupported".into());
    };
    let id: EntityId = format!("{}.{}.{}", id.shard_num, id.realm_num, n).parse()?;
    id.account()?;
    Ok(id)
}
fn token_id(id: Option<&pb::TokenId>) -> Result<EntityId> {
    let id = id.ok_or("missing token ID")?;
    format!("{}.{}.{}", id.shard_num, id.realm_num, id.token_num).parse()
}
fn signature_bytes(pair: &pb::SignaturePair) -> Result<&[u8]> {
    match &pair.signature {
        Some(
            pb::signature_pair::Signature::Ed25519(s)
            | pb::signature_pair::Signature::EcdsaSecp256k1(s),
        ) => Ok(s),
        _ => Err("unsupported signature".into()),
    }
}
fn validate_key(key: &pb::Key, depth: usize, leaves: &mut BTreeSet<Vec<u8>>) -> Result<()> {
    require!(
        depth <= 8 && leaves.len() < MAX_SIGNATURES,
        "account key tree too large"
    );
    match key.key.as_ref().ok_or("empty account key")? {
        pb::key::Key::Ed25519(k) | pb::key::Key::EcdsaSecp256k1(k) => {
            require!(
                leaves.insert(k.clone()),
                "duplicate leaf in account key tree"
            );
            PublicKey::from_bytes(k).map_err(|_| "invalid account public key")?;
        }
        pb::key::Key::KeyList(list) => {
            require!(
                !list.keys.is_empty() && list.keys.len() <= MAX_SIGNATURES,
                "invalid key list"
            );
            for key in &list.keys {
                validate_key(key, depth + 1, leaves)?;
            }
        }
        pb::key::Key::ThresholdKey(threshold) => {
            let keys = &threshold.keys.as_ref().ok_or("empty threshold key")?.keys;
            require!(
                threshold.threshold > 0
                    && threshold.threshold as usize <= keys.len()
                    && keys.len() <= MAX_SIGNATURES,
                "invalid key threshold"
            );
            for key in keys {
                validate_key(key, depth + 1, leaves)?;
            }
        }
        _ => return Err("contract or unsupported account key".into()),
    }
    Ok(())
}
fn satisfies(key: &pb::Key, variant: &Variant) -> Result<bool> {
    Ok(match key.key.as_ref().ok_or("empty key")? {
        pb::key::Key::Ed25519(k) | pb::key::Key::EcdsaSecp256k1(k) => {
            let public = PublicKey::from_bytes(k).map_err(|_| "invalid public key")?;
            variant
                .signed
                .sig_map
                .as_ref()
                .unwrap()
                .sig_pair
                .iter()
                .any(|p| {
                    p.pub_key_prefix == *k
                        && signature_bytes(p)
                            .is_ok_and(|s| public.verify(&variant.signed.body_bytes, s).is_ok())
                })
        }
        pb::key::Key::KeyList(list) => {
            let mut all = true;
            for key in &list.keys {
                all &= satisfies(key, variant)?;
            }
            all
        }
        pb::key::Key::ThresholdKey(t) => {
            let mut count = 0;
            for key in &t.keys.as_ref().unwrap().keys {
                if satisfies(key, variant)? {
                    count += 1;
                }
            }
            count >= t.threshold
        }
        _ => false,
    })
}
