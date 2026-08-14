//! The notarised evidence receipt, signed EIP-712 by the facilitator.
//!
//! What this buys, and what it does not:
//!
//! - It **does** let a third party check, offline and without calling us, that
//!   this facilitator attested that a given payment produced a given piece of
//!   evidence at a given pointer. That is exactly the property the IETF x402
//!   receipt drafts identify as missing from a bare `PAYMENT-RESPONSE`, where
//!   "an auditor cannot validate a retained receipt without contacting the
//!   facilitator".
//! - It does **not** prove the content is what the buyer wanted, or that the
//!   seller behaved. It proves what was anchored and when. `contentHash` is what
//!   ties the anchor to the bytes that were actually delivered.
//!
//! The receipt deliberately records `mode`. A `direct` receipt and an `escrowed`
//! receipt make materially different claims about who can read the payload, and
//! a verifier that could not tell them apart would over-trust the weaker one.

use alloy::primitives::{Address, FixedBytes, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};
use thiserror::Error;

use super::types::EvidenceReceipt;

sol! {
    /// EIP-712 payload for an evidence attestation.
    ///
    /// Field order is normative -- it is part of the type hash, so reordering
    /// silently invalidates every previously issued receipt.
    #[derive(Debug)]
    struct Dx402EvidenceReceipt {
        bytes32 paymentId;
        bytes32 contentHash;
        string  pointer;
        address payer;
        address payee;
        bytes32 txHash;
        uint8   mode;
        uint64  anchoredAt;
        uint64  retentionUntil;
    }
}

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error("field `{field}` is not a valid 32-byte hex value: {value}")]
    BadHash { field: &'static str, value: String },
    #[error("field `{field}` is not an EVM address: {value}")]
    BadAddress { field: &'static str, value: String },
    #[error("signing failed: {0}")]
    Signing(String),
    #[error("signature is malformed: {0}")]
    MalformedSignature(String),
}

fn parse_b256(field: &'static str, value: &str) -> Result<B256, ReceiptError> {
    value.parse::<B256>().map_err(|_| ReceiptError::BadHash {
        field,
        value: value.to_string(),
    })
}

fn parse_address(field: &'static str, value: &str) -> Result<Address, ReceiptError> {
    value
        .parse::<Address>()
        .map_err(|_| ReceiptError::BadAddress {
            field,
            value: value.to_string(),
        })
}

/// Convert the wire receipt into its EIP-712 struct form.
///
/// Non-EVM payers and payees are represented by the zero address: their real
/// identifiers do not fit the `address` type, and the authoritative binding for
/// those chains is `paymentId` plus `txHash`, both of which are carried here.
fn to_sol(receipt: &EvidenceReceipt) -> Result<Dx402EvidenceReceipt, ReceiptError> {
    let payer = receipt.payer.to_string();
    let payee = receipt.payee.to_string();
    Ok(Dx402EvidenceReceipt {
        paymentId: parse_b256("paymentId", &receipt.payment_id)?,
        contentHash: parse_b256("contentHash", &receipt.content_hash)?,
        pointer: receipt.pointer.as_str().to_string(),
        payer: parse_address("payer", &payer).unwrap_or(Address::ZERO),
        payee: parse_address("payee", &payee).unwrap_or(Address::ZERO),
        txHash: parse_b256("txHash", &receipt.tx_hash).unwrap_or(FixedBytes::ZERO),
        mode: receipt.mode.as_u8(),
        anchoredAt: receipt.anchored_at,
        retentionUntil: receipt.retention_until,
    })
}

/// The EIP-712 digest a receipt is signed over.
pub fn signing_hash(receipt: &EvidenceReceipt, chain_id: u64) -> Result<B256, ReceiptError> {
    let domain = eip712_domain! {
        name: "DX402 Evidence",
        version: "1",
        chain_id: chain_id,
    };
    Ok(to_sol(receipt)?.eip712_signing_hash(&domain))
}

/// Sign a receipt with the facilitator's key.
pub fn sign(
    receipt: &EvidenceReceipt,
    signer: &PrivateKeySigner,
    chain_id: u64,
) -> Result<String, ReceiptError> {
    let hash = signing_hash(receipt, chain_id)?;
    let sig = signer
        .sign_hash_sync(&hash)
        .map_err(|e| ReceiptError::Signing(e.to_string()))?;
    Ok(format!("0x{}", hex::encode(sig.as_bytes())))
}

/// Recover the address that signed a receipt.
///
/// A verifier compares this against the facilitator address published at
/// `/supported`. Anyone can run this; nothing here requires our cooperation,
/// which is the point.
pub fn recover_signer(
    receipt: &EvidenceReceipt,
    signature: &str,
    chain_id: u64,
) -> Result<Address, ReceiptError> {
    let raw = hex::decode(signature.trim_start_matches("0x"))
        .map_err(|e| ReceiptError::MalformedSignature(e.to_string()))?;
    let sig = alloy::primitives::Signature::try_from(raw.as_slice())
        .map_err(|e| ReceiptError::MalformedSignature(e.to_string()))?;
    let hash = signing_hash(receipt, chain_id)?;
    sig.recover_address_from_prehash(&hash)
        .map_err(|e| ReceiptError::MalformedSignature(e.to_string()))
}

/// Whether `signature` is a valid receipt signature by `expected`.
pub fn verify(
    receipt: &EvidenceReceipt,
    signature: &str,
    expected: Address,
    chain_id: u64,
) -> bool {
    recover_signer(receipt, signature, chain_id)
        .map(|a| a == expected)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx402::types::{DurablePointer, EvidenceMode};
    use crate::network::Network;
    use crate::types::MixedAddress;

    /// `MixedAddress` is parsed through `Deserialize`, not `FromStr`.
    fn addr(s: &str) -> MixedAddress {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
    }

    fn sample() -> EvidenceReceipt {
        EvidenceReceipt {
            payment_id: format!("0x{}", "11".repeat(32)),
            content_hash: format!("0x{}", "22".repeat(32)),
            pointer: DurablePointer("s3+https://evidence.ultravioletadao.xyz/e/a.dx402".into()),
            payer: addr("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
            payee: addr("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
            tx_hash: format!("0x{}", "33".repeat(32)),
            network: Network::Base,
            mode: EvidenceMode::Direct,
            anchored_at: 1_760_000_000,
            retention_until: 1_760_000_000 + 90 * 86_400,
        }
    }

    #[test]
    fn a_receipt_verifies_against_its_signer() {
        let signer = PrivateKeySigner::random();
        let receipt = sample();
        let sig = sign(&receipt, &signer, 8453).unwrap();
        assert!(verify(&receipt, &sig, signer.address(), 8453));
    }

    #[test]
    fn another_signer_does_not_verify() {
        let signer = PrivateKeySigner::random();
        let impostor = PrivateKeySigner::random();
        let receipt = sample();
        let sig = sign(&receipt, &signer, 8453).unwrap();
        assert!(!verify(&receipt, &sig, impostor.address(), 8453));
    }

    #[test]
    fn every_field_is_bound_into_the_signature() {
        // If a field were omitted from the type hash, an attacker could alter it
        // -- swap the pointer, extend the retention, downgrade the mode -- and
        // still present a valid-looking receipt.
        let signer = PrivateKeySigner::random();
        let base = sample();
        let sig = sign(&base, &signer, 8453).unwrap();

        let mutations: Vec<(&str, EvidenceReceipt)> = vec![
            (
                "paymentId",
                EvidenceReceipt {
                    payment_id: format!("0x{}", "99".repeat(32)),
                    ..base.clone()
                },
            ),
            (
                "contentHash",
                EvidenceReceipt {
                    content_hash: format!("0x{}", "99".repeat(32)),
                    ..base.clone()
                },
            ),
            (
                "pointer",
                EvidenceReceipt {
                    pointer: DurablePointer("s3+https://evil.example/x".into()),
                    ..base.clone()
                },
            ),
            (
                "txHash",
                EvidenceReceipt {
                    tx_hash: format!("0x{}", "99".repeat(32)),
                    ..base.clone()
                },
            ),
            (
                "mode",
                EvidenceReceipt {
                    mode: EvidenceMode::Escrowed,
                    ..base.clone()
                },
            ),
            (
                "anchoredAt",
                EvidenceReceipt {
                    anchored_at: 1,
                    ..base.clone()
                },
            ),
            (
                "retentionUntil",
                EvidenceReceipt {
                    retention_until: 0,
                    ..base.clone()
                },
            ),
        ];

        for (field, mutated) in mutations {
            assert!(
                !verify(&mutated, &sig, signer.address(), 8453),
                "mutating {field} did not invalidate the signature"
            );
        }
    }

    #[test]
    fn the_signature_is_bound_to_its_chain() {
        // A receipt lifted from a testnet settlement must not verify as mainnet
        // evidence.
        let signer = PrivateKeySigner::random();
        let receipt = sample();
        let sig = sign(&receipt, &signer, 8453).unwrap();
        assert!(!verify(&receipt, &sig, signer.address(), 84532));
    }

    #[test]
    fn non_evm_parties_degrade_to_the_zero_address() {
        // Solana and Stellar identifiers do not fit `address`. They must not
        // block signing -- paymentId and txHash carry the real binding.
        let signer = PrivateKeySigner::random();
        let receipt = EvidenceReceipt {
            payer: addr("F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq"),
            network: Network::Solana,
            ..sample()
        };
        let sig = sign(&receipt, &signer, 101).unwrap();
        assert!(verify(&receipt, &sig, signer.address(), 101));
    }

    #[test]
    fn malformed_signatures_are_rejected_not_panicked_on() {
        let receipt = sample();
        for bad in ["", "0x", "0xzz", "0x00"] {
            assert!(!verify(&receipt, bad, Address::ZERO, 8453));
        }
    }

    #[test]
    fn a_malformed_payment_id_is_an_error_not_a_silent_zero() {
        // Quietly signing over a zero paymentId would produce a receipt that
        // verifies but attests to nothing.
        let receipt = EvidenceReceipt {
            payment_id: "not-a-hash".into(),
            ..sample()
        };
        assert!(matches!(
            signing_hash(&receipt, 8453),
            Err(ReceiptError::BadHash {
                field: "paymentId",
                ..
            })
        ));
    }
}
