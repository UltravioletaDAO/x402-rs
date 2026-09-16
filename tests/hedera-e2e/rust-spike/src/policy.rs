//! The inspection the SDK does NOT do.
//!
//! `AnyTransaction::from_bytes` refuses several malformed lists on its own, and
//! the vectors show which. It does not, and should not, know anything about
//! x402: a transfer that spends an allowance, drags an NFT along, calls a hook
//! or debits the sponsor is a perfectly valid Hedera transaction. Those are
//! ours to reject, before we sign anything.
//!
//! This is a spike-grade implementation of plan 7.3 points 2, 4, 5 and 6,
//! written against the protobuf rather than the SDK's aggregated getters, and
//! it exists to prove the rules are expressible on the bytes we actually get.

use std::collections::BTreeSet;

use crate::{RawTransfer, RawVariant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInput {
    pub asset: String,
    pub amount: i64,
    pub pay_to: String,
    pub fee_payer: String,
    pub sender: String,
}

/// Every variant must express the same single payment, and that payment must be
/// one this facilitator is willing to sponsor.
pub fn check(variants: &[RawVariant], want: &PolicyInput) -> Result<(), String> {
    let first = variants.first().ok_or("no variants")?;

    // 7.3 point 2: all variants, one intent, one node each.
    let mut nodes = BTreeSet::new();
    for v in variants {
        if v.body_kind != "CryptoTransfer" {
            return Err(format!("variant {} is a {}, not a transfer", v.index, v.body_kind));
        }
        if v.transaction_id != first.transaction_id {
            return Err(format!(
                "variant {} carries a different transaction id ({:?} vs {:?})",
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

    // 7.3 point 1, on the signature map rather than the body. An EMPTY
    // pubKeyPrefix is a prefix of every public key: it makes hiero-sdk skip
    // adding our own signature (`sign_with` treats the signer as already
    // present) and it makes `verify_transaction` attempt a verification it was
    // never asked for. 0.45.0 happens to refuse these lists at decode, which is
    // a property of this version and not something to depend on.
    let shape = |v: &RawVariant| -> Vec<(Vec<u8>, &'static str)> {
        let mut s: Vec<_> =
            v.signatures.iter().map(|s| (s.prefix.clone(), s.algorithm)).collect();
        s.sort();
        s
    };
    let first_shape = shape(first);
    for v in variants {
        for sig in &v.signatures {
            if sig.prefix.is_empty() {
                return Err(format!("variant {}: signature with an empty public key prefix", v.index));
            }
            if sig.algorithm == "unknown" {
                return Err(format!("variant {}: signature of an unsupported type", v.index));
            }
        }
        if shape(v) != first_shape {
            return Err(format!("variant {} carries a different set of signers", v.index));
        }
    }

    // 7.3 point 4: nothing riding along.
    for v in variants {
        for t in v.hbar_transfers.iter().chain(v.token_transfers.iter().flat_map(|t| t.transfers.iter())) {
            if t.is_approval {
                return Err(format!("variant {}: isApproval on {}", v.index, t.account));
            }
            if let Some(h) = t.hook {
                return Err(format!("variant {}: {h} on {}", v.index, t.account));
            }
        }
        for tt in &v.token_transfers {
            if tt.nft_transfer_count > 0 {
                return Err(format!(
                    "variant {}: {} NFT transfer(s) of {}",
                    v.index, tt.nft_transfer_count, tt.token
                ));
            }
        }
    }

    // 7.3 points 5 and 6: exactly the debit and the credit, and never the sponsor.
    let is_hbar = want.asset == "0.0.0";
    let entries: &[RawTransfer] = if is_hbar {
        if !first.token_transfers.is_empty() {
            return Err("HBAR payment carries token transfers".into());
        }
        &first.hbar_transfers
    } else {
        if !first.hbar_transfers.is_empty() {
            return Err("HTS payment carries explicit HBAR transfers".into());
        }
        match first.token_transfers.as_slice() {
            [one] if one.token == want.asset => &one.transfers,
            [one] => return Err(format!("token {} is not the requested {}", one.token, want.asset)),
            other => return Err(format!("{} token transfer lists, expected 1", other.len())),
        }
    };

    if entries.len() != 2 {
        return Err(format!("{} transfer entries, expected exactly 2", entries.len()));
    }
    let mut seen = BTreeSet::new();
    for e in entries {
        if !seen.insert(e.account.clone()) {
            return Err(format!("account {} appears twice", e.account));
        }
        if e.account == want.fee_payer && e.amount < 0 && want.fee_payer != want.pay_to {
            return Err(format!("the sponsor {} is debited {}", e.account, e.amount));
        }
    }
    let debit = entries
        .iter()
        .find(|e| e.account == want.sender)
        .ok_or_else(|| format!("no entry for the sender {}", want.sender))?;
    let credit = entries
        .iter()
        .find(|e| e.account == want.pay_to)
        .ok_or_else(|| format!("no entry for the payee {}", want.pay_to))?;
    if credit.amount != want.amount {
        return Err(format!("payee receives {}, requirements say {}", credit.amount, want.amount));
    }
    if debit.amount != -want.amount {
        return Err(format!("sender is debited {}, requirements say {}", debit.amount, -want.amount));
    }
    Ok(())
}
