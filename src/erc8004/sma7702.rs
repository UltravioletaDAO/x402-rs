//! Local half of an ERC-1271 path for Alchemy `SemiModularAccount7702`.
//!
//! KarmaKadabra raters whose EOA is EIP-7702-delegated to that account fail
//! prepare/submit with `relay_foreign_delegation`. That rejection is
//! deliberate: re-pointing the account at `FeedbackDelegate` would break
//! PayBox gasless money-ops. This module does not lift it.
//!
//! What it does, with no RPC:
//!
//! - recognises the two `SemiModularAccount7702` implementations Alchemy
//!   publishes (same address on every EVM chain);
//! - rebuilds the replay-safe EIP-712 digest those accounts sign;
//! - checks that a `pack1271Signature` envelope for entity 0 and an EOA
//!   signer recovers to the account itself.
//!
//! What it does not do: call `isValidSignature`, read the fallback signer, or
//! admit the rater to the relay. A changed fallback signer still verifies
//! here and would fail on chain. Authorship stays an open design question;
//! see `docs/plans/sma-7702-erc1271-foreign-delegation.md`.

use alloy::primitives::{keccak256, Address, Signature, B256, U256};

/// `SemiModularAccount7702` v1.0.0.
///
/// Published by Alchemy as the same address on every EVM chain. Not read
/// from a rater's `eth_getCode` in this branch.
pub const SMA_7702_V1_0_0: Address =
    alloy::primitives::address!("69007702764179f14F51cdce752f4f775d74E139");

/// `SemiModularAccount7702` v1.1.0.
///
/// Alchemy's default for new EIP-7702 accounts from 2026-09-21. Existing
/// v1.0.0 delegations stay on [`SMA_7702_V1_0_0`].
pub const SMA_7702_V1_1_0: Address =
    alloy::primitives::address!("77021100bD87b7008E5E1989d0eB38555d0d0000");

/// `pack1271Signature` for entity 0 and an EOA signer, without the 65-byte
/// payload.
///
/// Layout, matching Alchemy's SDK and the KarmaKadabra wrap: `0x00`,
/// `uint32` entity id 0, `0xFF`, signature prefix `0x00` (EOA).
pub const SMA_1271_FALLBACK_EOA_PREFIX: [u8; 7] = [0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x00];

const REPLAY_SAFE_TYPE: &[u8] = b"ReplaySafeHash(bytes32 hash)";
const DOMAIN_TYPE: &[u8] = b"EIP712Domain(uint256 chainId,address verifyingContract)";

/// True when `target` is a published `SemiModularAccount7702` implementation.
///
/// Recognition is not admission. `delegation_state` still returns `Foreign`
/// for these addresses.
pub fn is_known_sma_7702_implementation(target: Address) -> bool {
    target == SMA_7702_V1_0_0 || target == SMA_7702_V1_1_0
}

/// The 20-byte delegate inside an EIP-7702 designator (`0xef0100 || address`).
///
/// Anything else — empty code, a real contract, a truncated designator — is
/// `None`. This does not classify the delegate.
pub fn eip7702_delegate(code: &[u8]) -> Option<Address> {
    if code.len() == 23 && code[0] == 0xef && code[1] == 0x01 && code[2] == 0x00 {
        Some(Address::from_slice(&code[3..23]))
    } else {
        None
    }
}

/// Replay-safe digest the SMA fallback signer must sign.
///
/// Mirrors `SemiModularAccount.replaySafeHash`: EIP-712 domain
/// `(chainId, verifyingContract=account)` and struct `ReplaySafeHash(bytes32
/// hash)`, with no name, version, or salt. `account` is the rater EOA. For a
/// 7702 SMA that has not moved its fallback signer, that EOA is the signer.
pub fn sma_replay_safe_hash(chain_id: u64, account: Address, inner: B256) -> B256 {
    let domain_typehash = keccak256(DOMAIN_TYPE);
    let mut domain_preimage = Vec::with_capacity(96);
    domain_preimage.extend_from_slice(domain_typehash.as_slice());
    domain_preimage.extend_from_slice(&U256::from(chain_id).to_be_bytes::<32>());
    domain_preimage.extend_from_slice(&[0u8; 12]);
    domain_preimage.extend_from_slice(account.as_slice());
    let domain_separator = keccak256(&domain_preimage);

    let struct_typehash = keccak256(REPLAY_SAFE_TYPE);
    let mut struct_preimage = Vec::with_capacity(64);
    struct_preimage.extend_from_slice(struct_typehash.as_slice());
    struct_preimage.extend_from_slice(inner.as_slice());
    let struct_hash = keccak256(&struct_preimage);

    let mut digest = Vec::with_capacity(2 + 64);
    digest.extend_from_slice(&[0x19, 0x01]);
    digest.extend_from_slice(domain_separator.as_slice());
    digest.extend_from_slice(struct_hash.as_slice());
    keccak256(&digest)
}

/// `Some(raw)` when `wrapped` is the entity-0 EOA envelope plus a 65-byte
/// signature and nothing else.
///
/// A contract-signer prefix (`0x01`), a non-zero entity id, or trailing hook
/// data is `None`. Those need the account's own `isValidSignature`.
pub fn unwrap_sma_1271_eoa_signature(wrapped: &[u8]) -> Option<&[u8]> {
    let (prefix, raw) = wrapped.split_at_checked(SMA_1271_FALLBACK_EOA_PREFIX.len())?;
    if prefix != SMA_1271_FALLBACK_EOA_PREFIX || raw.len() != 65 {
        return None;
    }
    Some(raw)
}

/// Does this envelope recover to `account` over [`sma_replay_safe_hash`]?
///
/// Local stand-in for the default 7702 fallback signer (`address(this)`),
/// not a verdict from the account. High-`s` signatures are rejected, same
/// rule as the relay's 7702 authority recovery.
pub fn sma_1271_eoa_authorises(
    chain_id: u64,
    account: Address,
    inner: B256,
    wrapped: &[u8],
) -> bool {
    let Some(raw) = unwrap_sma_1271_eoa_signature(wrapped) else {
        return false;
    };
    let Ok(sig) = Signature::try_from(raw) else {
        return false;
    };
    if sig.s() > alloy::eips::eip7702::constants::SECP256K1N_HALF {
        return false;
    }
    let digest = sma_replay_safe_hash(chain_id, account, inner);
    match sig.recover_address_from_prehash(&digest) {
        Ok(recovered) => recovered == account,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;

    fn pinned(hex_no_prefix: &str) -> B256 {
        assert_eq!(hex_no_prefix.len(), 64);
        let bytes = hex_no_prefix.as_bytes();
        let nibble = |c: u8| -> u8 {
            match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => panic!("not hex"),
            }
        };
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = (nibble(bytes[i * 2]) << 4) | nibble(bytes[i * 2 + 1]);
        }
        B256::from(out)
    }

    #[test]
    fn only_the_two_published_7702_implementations_are_known() {
        assert!(is_known_sma_7702_implementation(SMA_7702_V1_0_0));
        assert!(is_known_sma_7702_implementation(SMA_7702_V1_1_0));
        assert_ne!(SMA_7702_V1_0_0, SMA_7702_V1_1_0);
        // Bytecode and storage-only SMA variants are not 7702 delegates.
        let bytecode = alloy::primitives::address!("000000000000c5A9089039570Dd36455b5C07383");
        let storage = alloy::primitives::address!("0000000000006E2f9d80CaEc0Da6500f005EB25A");
        assert!(!is_known_sma_7702_implementation(bytecode));
        assert!(!is_known_sma_7702_implementation(storage));
        // Execution Market's Base FeedbackDelegate is not an SMA.
        let feedback_delegate =
            alloy::primitives::address!("260D3D0258680aA458D0EBB8BcAE8A2f68bf6163");
        assert!(!is_known_sma_7702_implementation(feedback_delegate));
        assert!(!is_known_sma_7702_implementation(Address::ZERO));
    }

    #[test]
    fn a_designator_yields_its_delegate_and_nothing_else_does() {
        let mut code = vec![0xef, 0x01, 0x00];
        code.extend_from_slice(SMA_7702_V1_1_0.as_slice());
        assert_eq!(eip7702_delegate(&code), Some(SMA_7702_V1_1_0));
        assert!(is_known_sma_7702_implementation(
            eip7702_delegate(&code).unwrap()
        ));

        assert_eq!(eip7702_delegate(&[]), None);
        assert_eq!(eip7702_delegate(&[0xef, 0x01, 0x00]), None);
        let mut long = code.clone();
        long.push(0x00);
        assert_eq!(eip7702_delegate(&long), None);
        assert_eq!(eip7702_delegate(&[0x60, 0x80]), None);
    }

    /// Typehashes published in `SemiModularAccount.sol`, and two full digests
    /// computed with an independent keccak (pycryptodome), not with this
    /// function. A formula compared only to itself is not a pin.
    #[test]
    fn the_replay_safe_digest_matches_the_independent_vectors() {
        assert_eq!(
            keccak256(DOMAIN_TYPE),
            pinned("47e79534a245952e8b16893a336b85a3d9ea9fa8c573f3d803afb92a79469218")
        );
        assert_eq!(
            keccak256(REPLAY_SAFE_TYPE),
            pinned("294a8735843d4afb4f017c76faf3b7731def145ed0025fc9b1d5ce30adf113ff")
        );

        let inner = pinned("1111111111111111111111111111111111111111111111111111111111111111");
        let account = alloy::primitives::address!("0000000000000000000000000000000000000001");
        assert_eq!(
            sma_replay_safe_hash(8453, account, inner),
            pinned("06b02e26f7d94d5b6e1a2b3d9e755c5f6e10331205ac0fcf1424865a7f8a3649")
        );

        let inner_ab = pinned("abababababababababababababababababababababababababababababababab");
        assert_eq!(
            sma_replay_safe_hash(1, SMA_7702_V1_0_0, inner_ab),
            pinned("df81465c93b7c6eb7da6e8aae643514ff35044377a092b8fc078fbed4f4b1a33")
        );
    }

    #[test]
    fn the_locator_is_entity_zero_and_the_eoa_prefix() {
        let mut prefix = [0u8; 7];
        prefix[0] = 0x00;
        prefix[1..5].copy_from_slice(&0u32.to_be_bytes());
        prefix[5] = 0xff;
        prefix[6] = 0x00;
        assert_eq!(SMA_1271_FALLBACK_EOA_PREFIX, prefix);
    }

    fn wrap(raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + raw.len());
        out.extend_from_slice(&SMA_1271_FALLBACK_EOA_PREFIX);
        out.extend_from_slice(raw);
        out
    }

    #[test]
    fn an_eoa_signature_over_the_replay_safe_digest_authorises_that_account() {
        let signer = PrivateKeySigner::random();
        let inner = B256::from([0x44; 32]);
        let digest = sma_replay_safe_hash(8453, signer.address(), inner);
        let raw = signer.sign_hash_sync(&digest).unwrap().as_bytes();
        let wrapped = wrap(&raw);
        assert!(sma_1271_eoa_authorises(
            8453,
            signer.address(),
            inner,
            &wrapped
        ));

        let stranger = PrivateKeySigner::random();
        assert!(!sma_1271_eoa_authorises(
            8453,
            stranger.address(),
            inner,
            &wrapped
        ));
        // Same key, different chain or different inner hash.
        assert!(!sma_1271_eoa_authorises(
            1,
            signer.address(),
            inner,
            &wrapped
        ));
        assert!(!sma_1271_eoa_authorises(
            8453,
            signer.address(),
            B256::from([0x45; 32]),
            &wrapped
        ));
    }

    #[test]
    fn a_bare_ecdsa_signature_is_not_the_sma_envelope() {
        let signer = PrivateKeySigner::random();
        let inner = B256::from([0x44; 32]);
        let digest = sma_replay_safe_hash(8453, signer.address(), inner);
        let raw = signer.sign_hash_sync(&digest).unwrap().as_bytes();
        assert!(unwrap_sma_1271_eoa_signature(&raw).is_none());
        assert!(!sma_1271_eoa_authorises(
            8453,
            signer.address(),
            inner,
            &raw
        ));
    }

    #[test]
    fn other_locators_are_refused() {
        let signer = PrivateKeySigner::random();
        let inner = B256::ZERO;
        let digest = sma_replay_safe_hash(10, signer.address(), inner);
        let raw = signer.sign_hash_sync(&digest).unwrap().as_bytes();

        // Contract-signer prefix instead of EOA.
        let mut contract_prefix = SMA_1271_FALLBACK_EOA_PREFIX;
        contract_prefix[6] = 0x01;
        let mut wrapped = contract_prefix.to_vec();
        wrapped.extend_from_slice(&raw);
        assert!(unwrap_sma_1271_eoa_signature(&wrapped).is_none());

        // Entity id 1 is not the reserved fallback signer.
        let mut other_entity = SMA_1271_FALLBACK_EOA_PREFIX;
        other_entity[4] = 0x01;
        let mut wrapped = other_entity.to_vec();
        wrapped.extend_from_slice(&raw);
        assert!(unwrap_sma_1271_eoa_signature(&wrapped).is_none());

        // Trailing byte: hook data this stub does not interpret.
        let mut wrapped = wrap(&raw);
        wrapped.push(0x00);
        assert!(unwrap_sma_1271_eoa_signature(&wrapped).is_none());
        assert!(!sma_1271_eoa_authorises(
            10,
            signer.address(),
            inner,
            &wrapped
        ));
    }

    #[test]
    fn a_high_s_encoding_is_rejected() {
        let signer = PrivateKeySigner::random();
        let inner = B256::from([0x07; 32]);
        let digest = sma_replay_safe_hash(8453, signer.address(), inner);
        let sig = signer.sign_hash_sync(&digest).unwrap();
        let n = U256::from_str_radix(
            "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
            16,
        )
        .unwrap();
        let mut raw = sig.as_bytes();
        raw[32..64].copy_from_slice(&(n - sig.s()).to_be_bytes::<32>());
        raw[64] ^= 1;
        let wrapped = wrap(&raw);
        assert!(!sma_1271_eoa_authorises(
            8453,
            signer.address(),
            inner,
            &wrapped
        ));
    }
}
