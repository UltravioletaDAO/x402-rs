//! Phase-0 Hedera spike: does the PUBLISHED Rust SDK let a facilitator co-sign
//! a payer's transaction without touching a single byte of what the payer
//! signed?
//!
//! The experiment, per vector:
//!
//!   1. read the base64 payload a real `@x402/hedera` client would send;
//!   2. inspect EVERY per-node variant at the protobuf level, not the first;
//!   3. decode it with `hiero-sdk` and check what the SDK reports;
//!   4. verify the payer's signature over the frozen bodies;
//!   5. co-sign as the facilitator;
//!   6. re-encode, re-decode, and compare byte for byte.
//!
//! Step 6 is the whole question. A codec that rebuilds the body from parsed
//! fields would produce something that still looks like the same payment and
//! carries a signature over bytes that no longer exist.

pub mod policy;

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use hiero_sdk::{AnyTransaction, PrivateKey, PublicKey, TransferTransaction};
use hiero_sdk_proto::services;
use prost::Message as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

// --------------------------------------------------------------------------
// Deterministic keys. Mirror of keys.mjs: SHA-256 over an ASCII label.
// Deriving the same keys on both sides, from a label rather than from stored
// key material, is what makes the cross-SDK comparison mean something.
// --------------------------------------------------------------------------

pub fn seed(label: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(label.as_bytes());
    h.finalize().into()
}

pub fn derive_ed25519(label: &str) -> Result<PrivateKey> {
    PrivateKey::from_bytes_ed25519(&seed(label)).context("ed25519 from seed")
}

pub fn derive_ecdsa(label: &str) -> Result<PrivateKey> {
    PrivateKey::from_bytes_ecdsa(&seed(label)).context("ecdsa from seed")
}

// --------------------------------------------------------------------------
// The vector files, as produced by generate-vectors.mjs.
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vector {
    pub name: String,
    pub description: String,
    pub network: String,
    pub payment_requirements: PaymentRequirements,
    pub keys: BTreeMap<String, serde_json::Value>,
    pub payload: Payload,
    pub expected: Expected,
    pub facilitator_must_co_sign: bool,
    pub adversarial: Option<Adversarial>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    pub asset: String,
    pub amount: String,
    pub pay_to: String,
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extra {
    pub fee_payer: String,
}

#[derive(Debug, Deserialize)]
pub struct Payload {
    pub transaction: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Expected {
    pub sender_account_id: String,
    pub pay_to: String,
    pub fee_payer: String,
    pub asset: String,
    pub amount: String,
    pub variant_count: usize,
    pub variants: Vec<ExpectedVariant>,
    #[serde(default)]
    pub sender_signs_every_variant: bool,
    #[serde(default)]
    pub divergent_variant_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedVariant {
    pub index: usize,
    pub body_sha256: String,
    pub body_length: usize,
    pub node_account_id: Option<String>,
    pub transaction_id: Option<String>,
    pub signatures: Vec<ExpectedSig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedSig {
    pub public_key_prefix_hex: String,
    pub algorithm: String,
    pub signature_hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Adversarial {
    pub kind: String,
    pub must_be_rejected_because: String,
}

pub fn load_vector(path: &std::path::Path) -> Result<Vector> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

pub fn vector_paths(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read_dir {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("json")
                && p.file_name().and_then(|s| s.to_str()) != Some("index.json")
        })
        .collect();
    out.sort();
    Ok(out)
}

pub fn vectors_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("rust-spike has a parent")
        .join("vectors")
}

// --------------------------------------------------------------------------
// Protobuf-level view. This is the part the SDK does not hand us: per-node
// bodyBytes, per-node signature pairs, and the fields the SDK's aggregated
// getters cannot express.
// --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSig {
    pub prefix: Vec<u8>,
    pub algorithm: &'static str,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RawVariant {
    pub index: usize,
    /// The exact bytes the payer signed. Never rebuilt, only carried.
    pub body_bytes: Vec<u8>,
    pub node_account_id: Option<String>,
    pub transaction_id: Option<String>,
    pub transaction_fee: u64,
    pub memo: String,
    pub body_kind: String,
    pub hbar_transfers: Vec<RawTransfer>,
    pub token_transfers: Vec<RawTokenTransfer>,
    pub signatures: Vec<RawSig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTransfer {
    pub account: String,
    pub amount: i64,
    pub is_approval: bool,
    pub hook: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTokenTransfer {
    pub token: String,
    pub expected_decimals: Option<u32>,
    pub transfers: Vec<RawTransfer>,
    pub nft_transfer_count: usize,
}

impl RawVariant {
    pub fn body_sha256(&self) -> String {
        let mut h = Sha256::new();
        h.update(&self.body_bytes);
        hex::encode(h.finalize())
    }
}

fn account_id_str(id: &Option<services::AccountId>) -> Option<String> {
    id.as_ref().map(|a| {
        let num = match &a.account {
            Some(services::account_id::Account::AccountNum(n)) => n.to_string(),
            Some(services::account_id::Account::Alias(bytes)) => format!("alias:{}", hex::encode(bytes)),
            None => "0".to_string(),
        };
        format!("{}.{}.{}", a.shard_num, a.realm_num, num)
    })
}

fn transaction_id_str(id: &Option<services::TransactionId>) -> Option<String> {
    id.as_ref().map(|t| {
        let acc = account_id_str(&t.account_id).unwrap_or_else(|| "?".into());
        let (s, n) = t
            .transaction_valid_start
            .as_ref()
            .map(|ts| (ts.seconds, ts.nanos))
            .unwrap_or((0, 0));
        format!("{acc}@{s}.{n:09}")
    })
}

fn transfer_of(a: &services::AccountAmount) -> RawTransfer {
    RawTransfer {
        account: account_id_str(&a.account_id).unwrap_or_else(|| "?".into()),
        amount: a.amount,
        is_approval: a.is_approval,
        hook: match &a.hook_call {
            Some(services::account_amount::HookCall::PreTxAllowanceHook(_)) => {
                Some("preTxAllowanceHook")
            }
            Some(services::account_amount::HookCall::PrePostTxAllowanceHook(_)) => {
                Some("prePostTxAllowanceHook")
            }
            None => None,
        },
    }
}

fn body_kind(body: &services::TransactionBody) -> String {
    match &body.data {
        Some(d) => format!("{d:?}")
            .split_once('(')
            .map(|(k, _)| k.to_string())
            .unwrap_or_else(|| "unknown".into()),
        None => "none".into(),
    }
}

/// Decode `TransactionList` -> per-node `SignedTransaction` -> `TransactionBody`,
/// keeping the body bytes verbatim.
pub fn parse_variants(bytes: &[u8]) -> Result<Vec<RawVariant>> {
    let list = hiero_sdk_proto::sdk::TransactionList::decode(bytes)
        .context("decode TransactionList")?;
    if list.transaction_list.is_empty() {
        bail!("empty transaction list");
    }
    let mut out = Vec::with_capacity(list.transaction_list.len());
    for (index, entry) in list.transaction_list.iter().enumerate() {
        let signed = services::SignedTransaction::decode(&*entry.signed_transaction_bytes)
            .with_context(|| format!("decode SignedTransaction {index}"))?;
        let body = services::TransactionBody::decode(&*signed.body_bytes)
            .with_context(|| format!("decode TransactionBody {index}"))?;

        let (hbar_transfers, token_transfers) = match &body.data {
            Some(services::transaction_body::Data::CryptoTransfer(ct)) => (
                ct.transfers
                    .as_ref()
                    .map(|t| t.account_amounts.iter().map(transfer_of).collect())
                    .unwrap_or_default(),
                ct.token_transfers
                    .iter()
                    .map(|t| RawTokenTransfer {
                        token: t
                            .token
                            .as_ref()
                            .map(|id| format!("{}.{}.{}", id.shard_num, id.realm_num, id.token_num))
                            .unwrap_or_else(|| "?".into()),
                        expected_decimals: t.expected_decimals,
                        transfers: t.transfers.iter().map(transfer_of).collect(),
                        nft_transfer_count: t.nft_transfers.len(),
                    })
                    .collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };

        let signatures = signed
            .sig_map
            .as_ref()
            .map(|m| {
                m.sig_pair
                    .iter()
                    .map(|p| {
                        let (algorithm, signature) = match &p.signature {
                            Some(services::signature_pair::Signature::Ed25519(s)) => {
                                ("ed25519", s.clone())
                            }
                            Some(services::signature_pair::Signature::EcdsaSecp256k1(s)) => {
                                ("ecdsa_secp256k1", s.clone())
                            }
                            _ => ("unknown", Vec::new()),
                        };
                        RawSig { prefix: p.pub_key_prefix.clone(), algorithm, signature }
                    })
                    .collect()
            })
            .unwrap_or_default();

        out.push(RawVariant {
            index,
            body_bytes: signed.body_bytes.to_vec(),
            node_account_id: account_id_str(&body.node_account_id),
            transaction_id: transaction_id_str(&body.transaction_id),
            transaction_fee: body.transaction_fee,
            memo: body.memo.clone(),
            body_kind: body_kind(&body),
            hbar_transfers,
            token_transfers,
            signatures,
        });
    }
    Ok(out)
}

// --------------------------------------------------------------------------
// The experiment.
// --------------------------------------------------------------------------

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub vector: String,
    /// Did the TS-produced bytes and the Rust protobuf view agree, per variant?
    pub cross_sdk_bodies_match: bool,
    /// Did `AnyTransaction::from_bytes` accept the payload?
    pub sdk_decode: Result3,
    pub sdk_decode_error: Option<String>,
    /// Did it downcast to a TransferTransaction?
    pub downcast_transfer: Result3,
    pub sdk_variant_count: Option<usize>,
    pub raw_variant_count: usize,
    /// Does the payer's key verify over every frozen body?
    pub payer_signature: Result3,
    pub payer_signature_error: Option<String>,
    /// from_bytes -> to_bytes with NO signing: are the bytes identical?
    pub roundtrip_identical: Result3,
    /// After co-signing: every body byte-identical, every prior signature kept.
    pub bodies_preserved_after_cosign: Result3,
    pub prior_signatures_preserved: Result3,
    pub cosignature_on_every_variant: Result3,
    pub cosignature_verifies: Result3,
    /// The inspection the SDK does not do: does this payload pass OUR policy?
    pub facilitator_policy: Result3,
    pub facilitator_policy_error: Option<String>,
    /// What the SDK's aggregated getters can see, versus the protobuf.
    pub sdk_getters_hide: Vec<String>,
    pub notes: Vec<String>,
}

/// Three-valued: the experiment must be able to say "did not get that far".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Result3 {
    Pass,
    Fail,
    NotReached,
}

impl Default for Result3 {
    fn default() -> Self {
        Self::NotReached
    }
}

impl Result3 {
    pub fn mark(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotReached => "n/a ",
        }
    }
    pub fn of(b: bool) -> Self {
        if b {
            Self::Pass
        } else {
            Self::Fail
        }
    }
}

fn public_key_from_record(v: &serde_json::Value) -> Option<PublicKey> {
    let der = v.get("publicKeyDer")?.as_str()?;
    PublicKey::from_str_der(der).ok()
}

/// Every public key the vector names as a signer of the account being debited.
fn payer_keys(vector: &Vector) -> Vec<(String, PublicKey)> {
    let mut out = Vec::new();
    if let Some(k) = vector.keys.get("sender").and_then(public_key_from_record) {
        out.push(("sender".to_string(), k));
    }
    if let Some(acct) = vector.keys.get("senderAccountKey") {
        if let Some(members) = acct.get("members").and_then(|m| m.as_array()) {
            let signed_by: Vec<String> = acct
                .get("signedBy")
                .and_then(|s| s.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            for m in members {
                let label = m.get("label").and_then(|l| l.as_str()).unwrap_or("");
                if !signed_by.is_empty() && !signed_by.iter().any(|s| s == label) {
                    continue;
                }
                if let Some(k) = public_key_from_record(m) {
                    out.push((label.to_string(), k));
                }
            }
        } else if let Some(k) = public_key_from_record(acct) {
            if out.is_empty() {
                out.push(("senderAccountKey".to_string(), k));
            }
        }
    }
    out
}

pub fn run(vector: &Vector) -> Result<Report> {
    let mut report = Report { vector: vector.name.clone(), ..Default::default() };

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(vector.payload.transaction.as_bytes())
        .context("base64 payload")?;

    // --- step 2: every variant, at the protobuf level -----------------------
    let raw_before = parse_variants(&bytes)?;
    report.raw_variant_count = raw_before.len();

    // Cross-SDK check: the bodies the Rust side sees must be, byte for byte,
    // the bodies the TypeScript side recorded when it produced them.
    report.cross_sdk_bodies_match = raw_before.len() == vector.expected.variants.len()
        && raw_before.iter().zip(&vector.expected.variants).all(|(r, e)| {
            r.body_sha256() == e.body_sha256
                && r.body_bytes.len() == e.body_length
                && r.node_account_id == e.node_account_id
                && r.transaction_id == e.transaction_id
                && r.signatures.len() == e.signatures.len()
                && r.signatures.iter().zip(&e.signatures).all(|(rs, es)| {
                    hex::encode(&rs.prefix) == es.public_key_prefix_hex
                        && rs.algorithm == es.algorithm
                        && hex::encode(&rs.signature) == es.signature_hex
                })
        });

    // The facilitator's own inspection, on the bytes, before any SDK involvement.
    let want = policy::PolicyInput {
        asset: vector.payment_requirements.asset.clone(),
        amount: vector.payment_requirements.amount.parse::<i64>().unwrap_or(-1),
        pay_to: vector.payment_requirements.pay_to.clone(),
        fee_payer: vector.payment_requirements.extra.fee_payer.clone(),
        sender: vector.expected.sender_account_id.clone(),
    };
    match policy::check(&raw_before, &want) {
        Ok(()) => report.facilitator_policy = Result3::Pass,
        Err(e) => {
            report.facilitator_policy = Result3::Fail;
            report.facilitator_policy_error = Some(e);
        }
    }

    // Fields the SDK's aggregated getters cannot express, read from protobuf.
    for v in &raw_before {
        for t in &v.hbar_transfers {
            if t.is_approval {
                report.sdk_getters_hide.push(format!("variant {}: isApproval on {}", v.index, t.account));
            }
            if let Some(h) = t.hook {
                report.sdk_getters_hide.push(format!("variant {}: {h} on {}", v.index, t.account));
            }
        }
        for tt in &v.token_transfers {
            if tt.nft_transfer_count > 0 {
                report.sdk_getters_hide.push(format!(
                    "variant {}: {} NFT transfer(s) of {}",
                    v.index, tt.nft_transfer_count, tt.token
                ));
            }
            for t in &tt.transfers {
                if t.is_approval {
                    report.sdk_getters_hide.push(format!(
                        "variant {}: isApproval on {} of {}",
                        v.index, t.account, tt.token
                    ));
                }
            }
        }
    }
    report.sdk_getters_hide.sort();
    report.sdk_getters_hide.dedup();

    // --- step 3: the SDK's own decode --------------------------------------
    let any = match AnyTransaction::from_bytes(&bytes) {
        Ok(t) => {
            report.sdk_decode = Result3::Pass;
            t
        }
        Err(e) => {
            report.sdk_decode = Result3::Fail;
            report.sdk_decode_error = Some(e.to_string());
            return Ok(report);
        }
    };

    let mut transfer: TransferTransaction = match any.downcast::<TransferTransaction>() {
        Ok(t) => {
            report.downcast_transfer = Result3::Pass;
            t
        }
        Err(_) => {
            report.downcast_transfer = Result3::Fail;
            return Ok(report);
        }
    };
    report.sdk_variant_count = transfer.get_node_account_ids().map(|n| n.len());

    // --- step 4: the payer's signature over every frozen body ---------------
    let keys = payer_keys(vector);
    if keys.is_empty() {
        report.notes.push("vector names no payer key".into());
    } else {
        let mut all_ok = true;
        let mut first_err = None;
        for (label, key) in &keys {
            if let Err(e) = key.verify_transaction(&mut transfer) {
                all_ok = false;
                first_err.get_or_insert(format!("{label}: {e}"));
            }
        }
        report.payer_signature = Result3::of(all_ok);
        report.payer_signature_error = first_err;
    }

    // --- step 5a: round trip with no signing at all -------------------------
    let roundtrip = transfer.to_bytes().context("to_bytes before co-signing")?;
    report.roundtrip_identical = Result3::of(roundtrip == bytes);
    if roundtrip != bytes {
        let rt = parse_variants(&roundtrip)?;
        let bodies_same = rt.len() == raw_before.len()
            && rt.iter().zip(&raw_before).all(|(a, b)| a.body_bytes == b.body_bytes);
        report.notes.push(format!(
            "outer encoding differs on re-serialize ({} -> {} bytes); body bytes identical: {}",
            bytes.len(),
            roundtrip.len(),
            bodies_same
        ));
    }

    // --- step 5b: co-sign as the facilitator --------------------------------
    let facilitator = derive_ed25519("x402-rs/hedera-spike/v1/ed25519/facilitator")?;
    let facilitator_pub = facilitator.public_key();
    transfer.sign(facilitator.clone());
    let after_bytes = transfer.to_bytes().context("to_bytes after co-signing")?;

    // --- step 6: re-decode and compare byte for byte ------------------------
    let raw_after = parse_variants(&after_bytes)?;

    let bodies_preserved = raw_after.len() == raw_before.len()
        && raw_after
            .iter()
            .zip(&raw_before)
            .all(|(a, b)| a.body_bytes == b.body_bytes);
    report.bodies_preserved_after_cosign = Result3::of(bodies_preserved);

    let prior_kept = raw_after.len() == raw_before.len()
        && raw_after.iter().zip(&raw_before).all(|(a, b)| {
            b.signatures.iter().all(|old| a.signatures.contains(old))
        });
    report.prior_signatures_preserved = Result3::of(prior_kept);

    let fac_prefix = facilitator_pub.to_bytes_raw();
    let mut added_everywhere = raw_after.len() == raw_before.len();
    let mut cosig_verifies = added_everywhere;
    for (a, b) in raw_after.iter().zip(&raw_before) {
        let new_sigs: Vec<&RawSig> = a
            .signatures
            .iter()
            .filter(|s| !b.signatures.contains(*s))
            .collect();
        if new_sigs.len() != 1 {
            added_everywhere = false;
            cosig_verifies = false;
            report
                .notes
                .push(format!("variant {}: {} new signature(s), expected 1", a.index, new_sigs.len()));
            continue;
        }
        let s = new_sigs[0];
        if !fac_prefix.starts_with(&s.prefix) {
            added_everywhere = false;
            report
                .notes
                .push(format!("variant {}: new signature is not the facilitator's", a.index));
        }
        if facilitator_pub.verify(&a.body_bytes, &s.signature).is_err() {
            cosig_verifies = false;
            report
                .notes
                .push(format!("variant {}: co-signature does not verify over that body", a.index));
        }
    }
    report.cosignature_on_every_variant = Result3::of(added_everywhere);
    report.cosignature_verifies = Result3::of(cosig_verifies);

    Ok(report)
}

/// Does every variant express the same payment? The SDK's own `from_bytes`
/// compares bodies but SKIPS `transaction_id` and `node_account_id`, so this
/// is a check a facilitator has to do for itself.
pub fn variants_agree(variants: &[RawVariant]) -> std::result::Result<(), String> {
    let first = variants.first().ok_or_else(|| "no variants".to_string())?;
    let mut nodes = std::collections::BTreeSet::new();
    for v in variants {
        if v.transaction_id != first.transaction_id {
            return Err(format!(
                "variant {} carries transaction id {:?}, variant 0 carries {:?}",
                v.index, v.transaction_id, first.transaction_id
            ));
        }
        if v.hbar_transfers != first.hbar_transfers || v.token_transfers != first.token_transfers {
            return Err(format!("variant {} moves different value", v.index));
        }
        if v.transaction_fee != first.transaction_fee || v.memo != first.memo {
            return Err(format!("variant {} differs in fee or memo", v.index));
        }
        let node = v
            .node_account_id
            .clone()
            .ok_or_else(|| format!("variant {} has no node account id", v.index))?;
        if !nodes.insert(node.clone()) {
            return Err(format!("node {node} appears in more than one variant"));
        }
    }
    Ok(())
}

pub fn decode_base64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| anyhow!("base64: {e}"))
}
