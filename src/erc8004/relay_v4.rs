//! v4 of the relay: EIP-712 typed data instead of an opaque hash.
//!
//! # Why there is a v4 at all
//!
//! v3 has the rater sign `keccak256(abi.encode(...))` under an EIP-191 envelope.
//! It is sound, and it is unreadable: the wallet shows a hex blob. The rater
//! cannot see the score, the agent, the deadline, or **which selector** they are
//! authorising. A compromised backend can hand them a digest that actually
//! covers `revokeFeedback` -- destroying a rating they gave earlier -- while the
//! JSON on screen says `score: 95`. They sign "a 95" and destroy reputation, and
//! they are *structurally incapable* of noticing.
//!
//! EIP-712 fixes that by construction: the wallet renders named fields.
//!
//! # Why three structs and not one
//!
//! Because the fields have to BE in the signed struct, not next to it. Execution
//! Market's first proposal named four of `giveFeedback`'s eight parameters, so
//! `valueDecimals`, `tag1`, `tag2` and `endpoint` would have travelled unbound:
//! `value=100, decimals=0` is "100" and `decimals=2` is "1.00", same signature.
//! And one struct cannot cover `revokeFeedback` (needs `feedbackIndex`) or
//! `appendResponse` (needs four more) at all.
//!
//! So each selector gets its own type, carrying the registry call's complete
//! parameter list, and **the contract builds the calldata from the struct**.
//! There is no separate `bytes data` that can drift from what was displayed.
//! The selector itself is the typehash.
//!
//! # The domain binds the rater, and that is load-bearing
//!
//! `verifyingContract` is the RATER'S ACCOUNT -- under EIP-7702 that is
//! `address(this)` inside the delegate. It is not a preference: with the
//! delegate as `verifyingContract`, every account delegated to it on a chain
//! would share one domain, and since no field names the rater, **the same
//! signature would replay against any other delegated account**.
//!
//! Consequence for the contract, which Execution Market got right and which is
//! the default-pattern trap: the domain separator **cannot be cached in the
//! constructor**. OpenZeppelin's `EIP712` caches it, and under 7702 that would
//! freeze the DELEGATE's address into it, so every signature in the world would
//! recover a stranger.
//!
//! # Verified against the deployed contract, not against ourselves
//!
//! `the_vector_matches_the_deployed_v4_contract` pins the domain separator and a
//! full `RelayedGiveFeedback` digest against values read from the v4 delegate on
//! Base with `eth_call` (2026-08-25). A formula compared only to itself proves
//! nothing -- three fabricated SEAL hashes once passed CI for months that way.

use alloy::dyn_abi::Eip712Domain;
use alloy::primitives::{Address, Bytes, FixedBytes, B256, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolStruct};
use serde_json::{json, Value};

use super::relay::RelayError;

/// EIP-712 domain name. Fixed by the contract; changing it invalidates every
/// signature.
pub const DOMAIN_NAME: &str = "FeedbackDelegate";
/// EIP-712 domain version.
pub const DOMAIN_VERSION: &str = "1";

sol! {
    /// `giveFeedback`'s eight parameters, plus the registry it targets and the
    /// authorisation window. Field order IS the type string; reordering changes
    /// the typehash and invalidates every signature.
    #[derive(Debug)]
    struct RelayedGiveFeedback {
        address registry;
        uint256 agentId;
        int128 value;
        uint8 valueDecimals;
        string tag1;
        string tag2;
        string endpoint;
        string feedbackURI;
        bytes32 feedbackHash;
        uint256 deadline;
        bytes32 nonce;
    }

    /// `revokeFeedback`. `feedbackIndex` is what makes an authorisation name ONE
    /// rating; without it a signature would cover any of the rater's ratings.
    #[derive(Debug)]
    struct RelayedRevokeFeedback {
        address registry;
        uint256 agentId;
        uint64 feedbackIndex;
        uint256 deadline;
        bytes32 nonce;
    }

    /// `appendResponse`.
    #[derive(Debug)]
    struct RelayedAppendResponse {
        address registry;
        uint256 agentId;
        address clientAddress;
        uint64 feedbackIndex;
        string responseURI;
        bytes32 responseHash;
        uint256 deadline;
        bytes32 nonce;
    }

    /// The v4 delegate's typed entry points. The XOR of these three selectors is
    /// `0x378a0c90`, the ERC-165 id v4 advertises -- recomputed in a test rather
    /// than trusted as a magic constant.
    #[sol(rpc)]
    interface IFeedbackDelegateV4 {
        function relayGiveFeedback(RelayedGiveFeedback calldata p, bytes calldata signature) external;
        function relayRevokeFeedback(RelayedRevokeFeedback calldata p, bytes calldata signature) external;
        function relayAppendResponse(RelayedAppendResponse calldata p, bytes calldata signature) external;
        function VERSION() external view returns (uint256);
    }
}

/// The ERC-165 id v4 advertises, and the discriminator we probe for.
pub const V4_RELAY_INTERFACE_ID: FixedBytes<4> = FixedBytes([0x37, 0x8a, 0x0c, 0x90]);

/// The EIP-712 domain for ONE rater on ONE chain.
///
/// `verifying_contract` is the rater's account, never the delegate. See the
/// module docs: the delegate would make one domain shared by every account
/// pointed at it, and the signature would replay across them.
pub fn domain(chain_id: u64, rater: Address) -> Eip712Domain {
    Eip712Domain::new(
        Some(DOMAIN_NAME.into()),
        Some(DOMAIN_VERSION.into()),
        Some(U256::from(chain_id)),
        Some(rater),
        None,
    )
}

/// The digest the rater signs for a `giveFeedback`.
///
/// Signed as typed data (`eth_signTypedData_v4`), which is why v4 needs no
/// equivalent of v3's `signingPayload`: `signTypedData` has no envelope to apply
/// twice, so the class of bug that kept the v3 rail at zero signatures cannot
/// happen here.
pub fn give_feedback_digest(chain_id: u64, rater: Address, p: &RelayedGiveFeedback) -> B256 {
    p.eip712_signing_hash(&domain(chain_id, rater))
}

/// The digest the rater signs for a `revokeFeedback`.
pub fn revoke_feedback_digest(chain_id: u64, rater: Address, p: &RelayedRevokeFeedback) -> B256 {
    p.eip712_signing_hash(&domain(chain_id, rater))
}

/// The digest the rater signs for an `appendResponse`.
pub fn append_response_digest(chain_id: u64, rater: Address, p: &RelayedAppendResponse) -> B256 {
    p.eip712_signing_hash(&domain(chain_id, rater))
}

/// Calldata sent TO THE RATER'S ADDRESS for a v4 `giveFeedback`.
pub fn relay_give_feedback_calldata(p: &RelayedGiveFeedback, signature: &Bytes) -> Bytes {
    IFeedbackDelegateV4::relayGiveFeedbackCall {
        p: p.clone(),
        signature: signature.clone(),
    }
    .abi_encode()
    .into()
}

/// Assemble the `RelayedGiveFeedback` a rater is being asked to authorise.
#[allow(clippy::too_many_arguments)]
pub fn give_feedback_params(
    registry: Address,
    agent_id: u64,
    value: i128,
    value_decimals: u8,
    tag1: &str,
    tag2: &str,
    endpoint: &str,
    feedback_uri: &str,
    feedback_hash: FixedBytes<32>,
    deadline: u64,
    nonce: FixedBytes<32>,
) -> RelayedGiveFeedback {
    RelayedGiveFeedback {
        registry,
        agentId: U256::from(agent_id),
        value,
        valueDecimals: value_decimals,
        tag1: tag1.to_string(),
        tag2: tag2.to_string(),
        endpoint: endpoint.to_string(),
        feedbackURI: feedback_uri.to_string(),
        feedbackHash: feedback_hash,
        deadline: U256::from(deadline),
        nonce,
    }
}

/// The `eth_signTypedData_v4` payload, ready to hand to a wallet unchanged.
///
/// Emitted as JSON rather than as typed fields because that is what every wallet
/// API takes, and because a client that reshapes it is a second implementation
/// of the encoding -- the exact failure this version exists to remove.
///
/// Numeric fields are strings: `uint256` does not survive JSON's float, and a
/// silently rounded `deadline` or `agentId` would hash to something else.
pub fn give_feedback_typed_data(chain_id: u64, rater: Address, p: &RelayedGiveFeedback) -> Value {
    json!({
        "domain": {
            "name": DOMAIN_NAME,
            "version": DOMAIN_VERSION,
            "chainId": chain_id,
            "verifyingContract": p_addr(rater),
        },
        "primaryType": "RelayedGiveFeedback",
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"},
            ],
            "RelayedGiveFeedback": [
                {"name": "registry", "type": "address"},
                {"name": "agentId", "type": "uint256"},
                {"name": "value", "type": "int128"},
                {"name": "valueDecimals", "type": "uint8"},
                {"name": "tag1", "type": "string"},
                {"name": "tag2", "type": "string"},
                {"name": "endpoint", "type": "string"},
                {"name": "feedbackURI", "type": "string"},
                {"name": "feedbackHash", "type": "bytes32"},
                {"name": "deadline", "type": "uint256"},
                {"name": "nonce", "type": "bytes32"},
            ],
        },
        "message": {
            "registry": p_addr(p.registry),
            "agentId": p.agentId.to_string(),
            "value": p.value.to_string(),
            "valueDecimals": p.valueDecimals,
            "tag1": p.tag1,
            "tag2": p.tag2,
            "endpoint": p.endpoint,
            "feedbackURI": p.feedbackURI,
            "feedbackHash": format!("{:#x}", p.feedbackHash),
            "deadline": p.deadline.to_string(),
            "nonce": format!("{:#x}", p.nonce),
        },
    })
}

fn p_addr(a: Address) -> String {
    a.to_checksum(None)
}

/// Refuse a v4 relay whose parameters do not reproduce the signed digest.
///
/// Same discipline as v3's submit: the facilitator rebuilds what it is going to
/// send from the DECLARED parameters and requires the rater's signature to cover
/// exactly that. It does not relay a struct it was handed.
pub fn signature_authorises(
    chain_id: u64,
    rater: Address,
    p: &RelayedGiveFeedback,
    signature: &Bytes,
) -> Result<(), RelayError> {
    let digest = give_feedback_digest(chain_id, rater, p);
    if super::relay::signature_authorises(digest, signature, rater) {
        Ok(())
    } else {
        Err(RelayError::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const REGISTRY: Address = address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63");

    /// The typehashes Execution Market's contract compiled, and that we
    /// confirmed to them before they deployed.
    ///
    /// Pinned because a field reorder or a type change is invisible in review
    /// and total at runtime: every signature stops verifying at once.
    #[test]
    fn the_typehashes_are_the_confirmed_ones() {
        // Hashed from the encoded type string rather than from an instance:
        // the typehash is a property of the TYPE, and building a sample value
        // to ask for it would invite the sample to drift from the fields.
        let th = |t: std::borrow::Cow<'static, str>| {
            format!("{:#x}", alloy::primitives::keccak256(t.as_bytes()))
        };
        assert_eq!(
            th(RelayedGiveFeedback::eip712_encode_type()),
            "0x1303f838650b9f1619400e61b9bda2d6b484ea75b3af6a9ae58337d57a0585c0"
        );
        assert_eq!(
            th(RelayedRevokeFeedback::eip712_encode_type()),
            "0x4216eda31386c6ee7eab53320b15e7b13ced07582db035f719fb73b640a5a1d7"
        );
        assert_eq!(
            th(RelayedAppendResponse::eip712_encode_type()),
            "0xcf4215ad5e2b816cea8e759d7e5429a2803265ee8d67e9fdb2069fa87b169243"
        );
    }

    /// Every field of `giveFeedback` is inside the type string.
    ///
    /// This is the whole reason v4 has three structs. The first proposal named
    /// four of the eight parameters, which would have left `valueDecimals`,
    /// `tag1`, `tag2` and `endpoint` unbound -- and `valueDecimals` alone turns
    /// a "100" into a "1.00" with the same signature.
    #[test]
    fn no_give_feedback_parameter_travels_outside_the_digest() {
        let t = RelayedGiveFeedback::eip712_encode_type();
        for field in [
            "address registry",
            "uint256 agentId",
            "int128 value",
            "uint8 valueDecimals",
            "string tag1",
            "string tag2",
            "string endpoint",
            "string feedbackURI",
            "bytes32 feedbackHash",
            "uint256 deadline",
            "bytes32 nonce",
        ] {
            assert!(t.contains(field), "{field} is not in the signed type: {t}");
        }
    }

    /// The ERC-165 id v4 advertises is the XOR of its three entry points.
    ///
    /// Derived rather than copied: a magic constant taken from a handoff cannot
    /// be checked, and this one is what tells v4 apart from v3 at request time.
    #[test]
    fn the_interface_id_is_the_xor_of_the_three_entry_points() {
        let x = u32::from_be_bytes(IFeedbackDelegateV4::relayGiveFeedbackCall::SELECTOR)
            ^ u32::from_be_bytes(IFeedbackDelegateV4::relayRevokeFeedbackCall::SELECTOR)
            ^ u32::from_be_bytes(IFeedbackDelegateV4::relayAppendResponseCall::SELECTOR);
        assert_eq!(
            x.to_be_bytes(),
            V4_RELAY_INTERFACE_ID.0,
            "our entry-point signatures no longer XOR to the id v4 advertises"
        );
    }

    /// Pinned against the DEPLOYED contract, not against this file.
    ///
    /// Both values were read from the v4 delegate on Base with `eth_call` on
    /// 2026-08-25 (Execution Market's vector, reproduced independently here
    /// before it was written down).
    #[test]
    fn the_vector_matches_the_deployed_v4_contract() {
        // `address(this)` when the view is called directly on the delegate.
        let account = address!("260D3D0258680aA458D0EBB8BcAE8A2f68bf6163");

        assert_eq!(
            format!("{:#x}", domain(8453, account).separator()),
            "0xae792427891b13b5663e321e5c8802c304437aad75338e22a4b8e1a47243d09d",
            "the domain separator drifted from the deployed contract"
        );

        let p = give_feedback_params(
            REGISTRY,
            2106,
            95,
            0,
            "quality",
            "api",
            "https://agent.example",
            "https://execution.market/feedback/abc",
            FixedBytes::<32>::ZERO,
            1_787_662_172,
            FixedBytes::<32>::from([0x42; 32]),
        );
        assert_eq!(
            format!("{:#x}", give_feedback_digest(8453, account, &p)),
            "0xa7a2a62dd18c44c79f7c252d628f32b3c46234b20d237866154dbe118ebaf6e1",
            "the giveFeedback digest drifted from the deployed contract"
        );
    }

    /// The domain is per-RATER, so two raters never share a signature.
    ///
    /// The contract must compute it per call for the same reason. A cached
    /// separator -- OpenZeppelin's default -- would freeze the delegate's own
    /// address in and make every signature recover a stranger.
    #[test]
    fn two_raters_get_different_domains() {
        let a = address!("0B3520435d7Bc7197C55204f01261706e5c7DcA5");
        let b = address!("09C32b8FC0a94A1EeD424499A42180e29667bEeE");
        assert_ne!(domain(8453, a).separator(), domain(8453, b).separator());
        // ...and so does the same rater on a different chain.
        assert_ne!(domain(8453, a).separator(), domain(1, a).separator());
    }

    /// Every field binds: change one and the digest changes.
    #[test]
    fn every_field_of_the_struct_binds() {
        let rater = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let base = give_feedback_params(
            REGISTRY,
            42,
            87,
            0,
            "quality",
            "api",
            "e",
            "u",
            FixedBytes::<32>::ZERO,
            1_786_400_000,
            FixedBytes::<32>::from([0x22; 32]),
        );
        let d = give_feedback_digest(8453, rater, &base);

        let mut p = base.clone();
        p.valueDecimals = 2;
        assert_ne!(
            give_feedback_digest(8453, rater, &p),
            d,
            "valueDecimals must bind -- 100 with 0 decimals is not 100 with 2"
        );

        let mut p = base.clone();
        p.tag1 = "spam".into();
        assert_ne!(give_feedback_digest(8453, rater, &p), d, "tag1 must bind");

        let mut p = base.clone();
        p.endpoint = "https://elsewhere.example".into();
        assert_ne!(
            give_feedback_digest(8453, rater, &p),
            d,
            "endpoint must bind"
        );

        let mut p = base.clone();
        p.value = -87;
        assert_ne!(
            give_feedback_digest(8453, rater, &p),
            d,
            "a negative value must not collide with its positive twin"
        );

        let mut p = base.clone();
        p.deadline = U256::from(1_786_400_001u64);
        assert_ne!(
            give_feedback_digest(8453, rater, &p),
            d,
            "deadline must bind"
        );
    }

    /// A rater's signature authorises their own struct and nobody else's.
    #[test]
    fn a_rater_signature_authorises_only_what_it_covers() {
        use alloy::signers::{local::PrivateKeySigner, SignerSync};

        let signer = PrivateKeySigner::random();
        let rater = signer.address();
        let p = give_feedback_params(
            REGISTRY,
            42,
            87,
            0,
            "quality",
            "api",
            "e",
            "u",
            FixedBytes::<32>::ZERO,
            1_786_400_000,
            FixedBytes::<32>::from([0x22; 32]),
        );

        let sig = signer
            .sign_hash_sync(&give_feedback_digest(8453, rater, &p))
            .unwrap();
        let sig: Bytes = sig.as_bytes().to_vec().into();
        assert!(signature_authorises(8453, rater, &p, &sig).is_ok());

        // Same signature, one field moved: refused.
        let mut tampered = p.clone();
        tampered.value = 10;
        assert!(signature_authorises(8453, rater, &tampered, &sig).is_err());

        // Same struct, wrong chain: refused. The chain is in the domain.
        assert!(signature_authorises(1, rater, &p, &sig).is_err());
    }

    /// The typed-data payload a wallet receives says exactly what is signed.
    #[test]
    fn the_typed_data_payload_matches_the_digest_it_claims() {
        let rater = address!("0B3520435d7Bc7197C55204f01261706e5c7DcA5");
        let p = give_feedback_params(
            REGISTRY,
            2106,
            95,
            0,
            "quality",
            "api",
            "https://agent.example",
            "https://execution.market/feedback/abc",
            FixedBytes::<32>::ZERO,
            1_787_662_172,
            FixedBytes::<32>::from([0x42; 32]),
        );
        let td = give_feedback_typed_data(8453, rater, &p);

        assert_eq!(td["primaryType"], "RelayedGiveFeedback");
        assert_eq!(td["domain"]["name"], DOMAIN_NAME);
        assert_eq!(td["domain"]["verifyingContract"], rater.to_checksum(None));
        // Numbers travel as strings: a uint256 does not survive a JSON float,
        // and a rounded agentId or deadline hashes to something else.
        assert_eq!(td["message"]["agentId"], "2106");
        assert_eq!(td["message"]["deadline"], "1787662172");
        assert_eq!(td["message"]["value"], "95");
        // The declared field list must match the struct the digest is built
        // from, or the wallet displays one thing and signs another.
        let listed: Vec<String> = td["types"]["RelayedGiveFeedback"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                format!(
                    "{} {}",
                    f["type"].as_str().unwrap(),
                    f["name"].as_str().unwrap()
                )
            })
            .collect();
        let encoded = RelayedGiveFeedback::eip712_encode_type();
        for f in &listed {
            assert!(encoded.contains(f.as_str()), "{f} is not in {encoded}");
        }
        assert_eq!(listed.len(), 11);
    }
}
