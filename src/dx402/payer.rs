//! Bridging an x402 payment payload to the payer's encryption key.
//!
//! This is where the abstract claim in [`super::pubkey`] -- "the payment already
//! contains the key" -- meets the concrete payload types.
//!
//! Two very different routes, by curve:
//!
//! - **ed25519 families** (Solana, NEAR, Stellar, Algorand): the payer *address*
//!   is the public key. Nothing has to be recovered, parsed or resolved.
//! - **EVM**: the key has to be recovered from the EIP-3009 signature, which
//!   means reconstructing the exact EIP-712 digest the payer signed -- including
//!   the token's domain name and version, which vary per chain.
//!
//! For the EVM digest this deliberately reuses
//! [`crate::chain::evm::find_known_eip712_metadata`] rather than carrying its
//! own table. A second copy would drift, and a drifted domain does not error --
//! it recovers a *different, perfectly valid* public key, and the body would be
//! sealed to a stranger while every log line said success.

use alloy::primitives::{Address, FixedBytes};
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};

use super::envelope::PayerPublicKey;
use super::pubkey;
use super::types::SkipReason;
use crate::network::Network;
use crate::types::{ExactPaymentPayload, PaymentPayload};

sol! {
    /// EIP-3009 authorization, matching what the payer signed.
    struct TransferWithAuthorization {
        address from;
        address to;
        uint256 value;
        uint256 validAfter;
        uint256 validBefore;
        bytes32 nonce;
    }
}

/// Derive the key a response body should be sealed to.
///
/// Takes the settled payer as reported by the facilitator rather than digging it
/// out of the payload. For ed25519 chains that address *is* the key, and it is
/// the one identity both sides already agree on after settlement.
///
/// Returns a [`SkipReason`] rather than an error: no recoverable key is a
/// perfectly normal outcome (smart-contract wallets have none), and it must
/// degrade to "no evidence this time", never to a failed payment.
pub fn payer_public_key(
    payload: &PaymentPayload,
    requirements: &crate::types::PaymentRequirements,
    settled_payer: &crate::types::MixedAddress,
) -> Result<PayerPublicKey, SkipReason> {
    // ed25519 families first: the address is the key, so these need no payload
    // inspection and no signature arithmetic whatsoever.
    match settled_payer {
        crate::types::MixedAddress::Solana(pk) => {
            return pubkey::from_solana_address(&pk.to_string()).map_err(|_| SkipReason::NoPayerKey)
        }
        crate::types::MixedAddress::Stellar(addr) => {
            return pubkey::from_stellar_address(addr).map_err(|_| SkipReason::NoPayerKey)
        }
        crate::types::MixedAddress::Algorand(addr) => {
            return pubkey::from_algorand_address(addr).map_err(|_| SkipReason::NoPayerKey)
        }
        _ => {}
    }

    match &payload.payload {
        ExactPaymentPayload::Evm(evm) => evm_payer_key(payload.network, evm, requirements),
        // NEAR needs the account's access key, which is an RPC lookup rather
        // than something carried in the payload; Sui and XRPL carry the key
        // inside a signature envelope this function does not parse yet.
        _ => Err(SkipReason::NoPayerKey),
    }
}

/// Recover an EVM payer's secp256k1 key from their EIP-3009 signature.
fn evm_payer_key(
    network: Network,
    payment: &crate::types::ExactEvmPayload,
    requirements: &crate::types::PaymentRequirements,
) -> Result<PayerPublicKey, SkipReason> {
    let auth = &payment.authorization;

    // The asset lives in the requirements, not the payload.
    let asset: Address = requirements
        .asset
        .clone()
        .try_into()
        .map_err(|_| SkipReason::NoPayerKey)?;

    // Static table first, exactly as `assert_domain` does on the verify path.
    // An unknown token means we cannot reconstruct the digest, so we skip rather
    // than guess.
    let (name, version) = crate::chain::evm::find_known_eip712_metadata(network, &asset)
        .ok_or(SkipReason::NoPayerKey)?;

    let chain_id = super::service::chain_id_of(network);
    if chain_id == 0 {
        return Err(SkipReason::NoPayerKey);
    }

    let domain = eip712_domain! {
        name: name,
        version: version,
        chain_id: chain_id,
        verifying_contract: asset,
    };

    let from: Address = auth.from.into();
    let to: Address = auth.to.into();
    let message = TransferWithAuthorization {
        from,
        to,
        value: auth.value.into(),
        validAfter: auth.valid_after.into(),
        validBefore: auth.valid_before.into(),
        nonce: FixedBytes(auth.nonce.0),
    };
    let digest = message.eip712_signing_hash(&domain);

    // A smart-contract wallet (EIP-1271 / EIP-6492) has no EOA key behind it, so
    // recovery is meaningless there. Those signatures are not 65 bytes and fall
    // out here as a skip.
    pubkey::from_evm_signature(&payment.signature.0, &digest.0, Some(&from))
        .map_err(|_| SkipReason::NoPayerKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_signature_is_a_skip_not_an_error() {
        // EIP-1271 and EIP-6492 signatures are not 65 bytes. Smart wallets must
        // degrade to "no evidence", never to a failed payment.
        assert!(matches!(
            pubkey::from_evm_signature(&[0u8; 96], &[0u8; 32], None),
            Err(pubkey::PubKeyError::BadSignatureLength(96))
        ));
    }

    #[test]
    fn non_evm_chain_ids_are_reported_as_zero() {
        // A zero chain id means the EIP-712 digest cannot be reconstructed, so
        // the EVM path must bail rather than sign over a bogus domain.
        assert_eq!(super::super::service::chain_id_of(Network::Base), 8453);
        assert_eq!(super::super::service::chain_id_of(Network::Solana), 0);
    }
}
