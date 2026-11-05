//! The shared policy vector of the escrow lifecycle `declared_signer` role,
//! `tests/fixtures/escrow/lifecycle-declared-signer-vector.json`.
//!
//! Made to be loaded byte for byte by both sides of the role: this verifier and
//! the Execution Market lifecycle signer. Its verdicts are policy; its digests and signatures were
//! computed by eth_account (`scripts/lifecycle_declared_signer_vector.py`), not
//! by this code, so the Rust side is never compared to itself. Every key is
//! derived from a public seed: test keys only.
//!
//! This module only reads the file. The tests that hold the verifier to it are
//! in `lifecycle_auth`; the one that holds `/supported` to it is in
//! `facilitator_local`.

use std::collections::BTreeMap;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use serde::Deserialize;

use crate::network::Network;

use super::addresses::{LifecycleSigner, OperatorAddresses};
use super::lifecycle_auth::{LifecycleAction, LifecycleAuth};
use super::types::{EscrowLifecyclePayload, EscrowPaymentInfo};

pub(crate) const FIXTURE: &str =
    include_str!("../../tests/fixtures/escrow/lifecycle-declared-signer-vector.json");

/// SHA-256 of the fixture as git stores it (LF line endings). The generator is
/// deterministic -- fixed seeds, RFC 6979 signatures, nonces derived from the
/// case ids -- so every run writes these exact bytes, and the other side
/// compares the same digest of its copy. Regenerating the vector on purpose
/// changes this constant with it.
pub(crate) const FIXTURE_SHA256: &str =
    "648acb9fa7fca1da2dcd012f8dec07fe94933a6ac96796ba588e7fa0cbf53e34";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Vector {
    pub vector_version: u32,
    pub announcement: AnnouncementSpec,
    pub now: u64,
    pub max_deadline_secs: u64,
    pub keys: BTreeMap<String, Key>,
    pub tables: BTreeMap<String, Vec<Row>>,
    /// table -> CAIP-2 network -> `{lifecycleSigners, lifecycleSignersVersion}`.
    pub announcements: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnnouncementSpec {
    pub lifecycle_signers_version: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Key {
    pub seed: String,
    pub address: Address,
}

impl Key {
    /// The key itself: keccak256 of the seed.
    pub fn signer(&self) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&keccak256(self.seed.as_bytes()))
            .expect("keccak256 of the seed is a valid secp256k1 key")
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Row {
    pub network: String,
    pub operator: Address,
    pub signer: Address,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaymentInfoJson {
    pub operator: Address,
    pub payer: Address,
    pub receiver: Address,
    pub token: Address,
    pub max_amount: String,
    pub pre_approval_expiry: u64,
    pub authorization_expiry: u64,
    pub refund_expiry: u64,
    pub min_fee_bps: u16,
    pub max_fee_bps: u16,
    pub fee_receiver: Address,
    pub salt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignedOver {
    pub chain_id: u64,
    pub amount: String,
}

/// `{verdict, role}` or `{step}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Outcome {
    #[serde(default)]
    pub verdict: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Case {
    pub id: String,
    pub table: String,
    pub network: String,
    pub action: String,
    pub payment_info: PaymentInfoJson,
    pub amount: String,
    pub deadline: u64,
    pub nonce: String,
    pub signer: String,
    pub claimed_signer: String,
    pub signed_over: SignedOver,
    pub owner_rpc: String,
    pub policy: Outcome,
    pub expected: Vec<Outcome>,
    pub em_delegate: Option<bool>,
    pub digest: String,
    pub signature: String,
}

pub(crate) fn vector() -> Vector {
    serde_json::from_str(FIXTURE).expect("the lifecycle vector is valid JSON of this shape")
}

/// A CAIP-2 id of the vector as a `Network`.
pub(crate) fn network(caip2: &str) -> Network {
    Network::from_caip2(caip2).unwrap_or_else(|| panic!("{caip2} is not a known network"))
}

impl Vector {
    /// The operator table with `name`'s declarations in place of the real
    /// (empty) lifecycle signer column, every other field as production has it.
    pub fn table(&self, name: &str) -> impl Fn(Network) -> Option<OperatorAddresses> {
        let rows: Vec<(Network, LifecycleSigner)> = self
            .tables
            .get(name)
            .unwrap_or_else(|| panic!("no table {name}"))
            .iter()
            .map(|r| {
                (
                    network(&r.network),
                    LifecycleSigner {
                        operator: r.operator,
                        signer: r.signer,
                    },
                )
            })
            .collect();
        move |n| {
            OperatorAddresses::for_network(n).map(|mut addrs| {
                addrs.lifecycle_signers = rows
                    .iter()
                    .filter(|(rn, _)| *rn == n)
                    .map(|(_, row)| *row)
                    .collect();
                addrs
            })
        }
    }

    pub fn key(&self, name: &str) -> &Key {
        self.keys
            .get(name)
            .unwrap_or_else(|| panic!("no key {name}"))
    }
}

fn hex32(s: &str) -> B256 {
    let raw = hex::decode(s).expect("hex");
    B256::from_slice(&raw)
}

impl Case {
    pub fn network(&self) -> Network {
        network(&self.network)
    }

    pub fn action(&self) -> LifecycleAction {
        match self.action.as_str() {
            "release" => LifecycleAction::Release,
            "refundInEscrow" => LifecycleAction::RefundInEscrow,
            other => panic!("unknown action {other}"),
        }
    }

    pub fn nonce(&self) -> B256 {
        hex32(&self.nonce)
    }

    pub fn digest(&self) -> B256 {
        hex32(&self.digest)
    }

    pub fn amount(&self) -> u128 {
        self.amount.parse().expect("amount")
    }

    /// The request body's `payload`, as the facilitator parses it.
    pub fn payload(&self, v: &Vector) -> EscrowLifecyclePayload {
        let p = &self.payment_info;
        let salt = U256::from_str_radix(&p.salt, 10).expect("salt");
        EscrowLifecyclePayload {
            payment_info: EscrowPaymentInfo {
                operator: p.operator,
                receiver: p.receiver,
                token: p.token,
                max_amount: p.max_amount.parse().expect("maxAmount"),
                pre_approval_expiry: p.pre_approval_expiry,
                authorization_expiry: p.authorization_expiry,
                refund_expiry: p.refund_expiry,
                min_fee_bps: p.min_fee_bps,
                max_fee_bps: p.max_fee_bps,
                fee_receiver: p.fee_receiver,
                salt: B256::from(salt.to_be_bytes::<32>()),
            },
            payer: p.payer,
            amount: self.amount(),
            lifecycle_auth: Some(LifecycleAuth {
                signer: v.key(&self.claimed_signer).address,
                deadline: self.deadline,
                nonce: self.nonce(),
                signature: Bytes::from(hex::decode(&self.signature).expect("signature hex")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// The vector is byte for byte the one both sides pinned: a hand edit, a
    /// different generator run or a copy that drifted turns this red. A
    /// Windows checkout reads the file with CRLF; git, and the other side's
    /// copy, hold it with LF.
    #[test]
    fn the_shared_vector_is_the_pinned_bytes() {
        let bytes = FIXTURE.replace("\r\n", "\n");
        assert_eq!(
            hex::encode(Sha256::digest(bytes.as_bytes())),
            FIXTURE_SHA256
        );
    }
}
