//! EIP-7702 relayed feedback: the rater is the author, the facilitator pays.
//!
//! # The problem this closes
//!
//! The ERC-8004 Reputation Registry records `msg.sender` as the author of every
//! feedback. There is no delegation path in the deployed contract -- no
//! `giveFeedbackWithSignature`, no ERC-2771 forwarder, nothing (verified against
//! the deployed implementation: 29 selectors, zero meta-transaction support). So
//! a rating the facilitator relays is a rating the chain attributes to the
//! FACILITATOR. On Base that is 87,2% of all feedback, 1384 entries, and the
//! same address can unilaterally revoke any of it.
//!
//! With EIP-7702 the rater delegates their own EOA to Execution Market's
//! `FeedbackDelegate`. We then send the transaction TO THE RATER'S ADDRESS, so
//! when that code calls the registry the `msg.sender` observed there is the
//! rater. We pay the gas and sign the outer transaction; we can never author or
//! revoke a rating.
//!
//! # What we refuse to do
//!
//! The same discipline as the SVM path: the facilitator does not relay what it
//! is handed. It rebuilds the registry calldata from the declared feedback
//! parameters and requires the rater's signature to cover exactly that. The
//! delegate itself is the second line -- it accepts only two selectors and can
//! never move funds -- but a facilitator that relays arbitrary calldata would be
//! trusting a contract audit to make up for its own missing check.
//!
//! # Deadlines are short on purpose
//!
//! `relayFeedback` is permissionless by design (anyone may relay a signed
//! authorisation; that is what lets us sponsor it). The mitigation agreed with
//! Execution Market for that is ours to apply: emit SHORT deadlines, so a signed
//! authorisation that leaks is live for minutes rather than forever.

use alloy::eips::eip7702::{Authorization, SignedAuthorization};
use alloy::primitives::{keccak256, Address, Bytes, FixedBytes, Signature, B256, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;

use crate::erc8004::abi::IReputationRegistry;
use crate::network::Network;

sol! {
    /// Execution Market's `FeedbackDelegate`, the EIP-7702 delegate a rater
    /// points their EOA at.
    #[sol(rpc)]
    interface IFeedbackDelegate {
        /// Relay a registry call authored by this account.
        function relayFeedback(
            bytes calldata data,
            uint256 deadline,
            bytes32 nonce,
            bytes calldata signature
        ) external;

        /// Whether an authorisation nonce has already been spent.
        function nonceUsed(bytes32 nonce) external view returns (bool);

        /// The registry this delegate is pinned to. Immutable, set at
        /// construction, no setter and no upgrade.
        function REPUTATION_REGISTRY() external view returns (address);
    }
}

/// How long a relay authorisation stays valid.
pub const ENV_RELAY_DEADLINE_SECS: &str = "ERC8004_RELAY_DEADLINE_SECS";

/// Fifteen minutes. Long enough for a human to sign in a wallet, short enough
/// that a leaked authorisation is not a standing permission.
pub const DEFAULT_RELAY_DEADLINE_SECS: u64 = 900;

/// Deployed `FeedbackDelegate` per network.
///
/// The delegate takes the registry address through its constructor
/// (`immutable`, no setter), because mainnets share one CREATE2 registry address
/// and testnets use another. So the delegate address differs per chain and each
/// one has to be handed to us after its deploy -- an entry invented here would
/// be an address with no contract behind it, which is exactly how the upto proxy
/// once produced fake-success settles.
///
/// Every address below was read off the chain before it was written down, on two
/// independent RPC endpoints each (2026-08-23): the address has code, its
/// `REPUTATION_REGISTRY()` reads back that network's registry, and the registry
/// itself has code there. The eight mainnet deploys are Execution Market's,
/// delivered in `contracts/deployments/feedback-delegate.json`; all eight carry
/// byte-identical runtime code (1996 bytes, sha256 `82adb6272f0f3f88...`), which
/// is what the shared mainnet registry immutable predicts.
///
/// | network | delegate | registry read back |
/// |---|---|---|
/// | base | `0x754206C4...68BA4` | `0x8004BAa1...9b63` |
/// | ethereum | `0xbeCeA467...57dd9` | `0x8004BAa1...9b63` |
/// | polygon | `0xf670C69B...46A7f` | `0x8004BAa1...9b63` |
/// | arbitrum | `0x794C907F...A31bA` | `0x8004BAa1...9b63` |
/// | bsc | `0x9551263b...eF787` | `0x8004BAa1...9b63` |
/// | optimism | `0x825E997F...2b82` | `0x8004BAa1...9b63` |
/// | monad | `0x825E997F...2b82` | `0x8004BAa1...9b63` |
/// | celo | `0xe25cF9B9...96B59` | `0x8004BAa1...9b63` |
/// | base-sepolia | `0x3A680854...d3768` | `0x8004B663...8713` |
///
/// Optimism and Monad share an address because CREATE2 from the same deployer,
/// salt and init code lands on the same address on both -- not a copy-paste
/// error. It is still checked per chain at request time.
///
/// **Avalanche is absent and that is not a "not yet".** Its C-Chain rejects the
/// transaction type outright (`-32000 transaction type not supported`, measured
/// by Execution Market's `rehearse_7702.py`), so there is nothing to deploy
/// against. Reputation for tasks paid on Avalanche is routed to another chain by
/// Execution Market instead; the payment stays on Avalanche. Do not add an entry
/// here until a C-Chain upgrade actually ships EIP-7702.
///
/// Scroll and SKALE Base are absent too: ERC-8004 is served there, but no
/// delegate has been deployed on either (SKALE's EVM predates Shanghai).
fn delegate_address(network: &Network) -> Option<Address> {
    match network {
        // Mainnets -- Execution Market deploys, verified on-chain 2026-08-23.
        Network::Base => Some(alloy::primitives::address!(
            "754206C4247317768bD86459E829a174d9C68BA4"
        )),
        Network::Ethereum => Some(alloy::primitives::address!(
            "beCeA4673C0105aF63d02688Be6DE6CA51D57dd9"
        )),
        Network::Polygon => Some(alloy::primitives::address!(
            "f670C69BCbb2453FaE5Ec009c2b6dd934BE46A7f"
        )),
        Network::Arbitrum => Some(alloy::primitives::address!(
            "794C907FdfC71BFaF0b86D0e463BBD6E949A31bA"
        )),
        Network::Bsc => Some(alloy::primitives::address!(
            "9551263b9B83b1A737D55fd5e67Fb6D60e4eF787"
        )),
        Network::Optimism => Some(alloy::primitives::address!(
            "825E997F2F7Ed5d3F59466cd754189fb19b62b82"
        )),
        Network::Monad => Some(alloy::primitives::address!(
            "825E997F2F7Ed5d3F59466cd754189fb19b62b82"
        )),
        Network::Celo => Some(alloy::primitives::address!(
            "e25cF9B9F5A3B5faa7628c751466df0166d96B59"
        )),
        // Testnet.
        Network::BaseSepolia => Some(alloy::primitives::address!(
            "3A68085499B62286468A35b7D9Dfc237ef2d3768"
        )),
        _ => None,
    }
}

/// The delegate for `network`, if relayed feedback is available there.
pub fn feedback_delegate(network: &Network) -> Option<Address> {
    delegate_address(network)
}

/// Deadline window, from the environment or [`DEFAULT_RELAY_DEADLINE_SECS`].
pub fn relay_deadline_secs() -> u64 {
    std::env::var(ENV_RELAY_DEADLINE_SECS)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_RELAY_DEADLINE_SECS)
}

/// What the rater's account currently looks like on chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationState {
    /// A plain EOA: no code. Needs an authorisation to be installed.
    None,
    /// Already delegated to the delegate we expect.
    Delegated,
    /// Delegated somewhere else, or a contract account. Not usable.
    Foreign,
}

/// Errors that stop a relay before any gas is spent.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("relayed feedback is not available on {0}: no FeedbackDelegate deployed there")]
    NoDelegate(Network),
    #[error("the rater's signature does not authorise this feedback")]
    BadSignature,
    #[error("the rater's account is delegated to something other than the expected delegate")]
    ForeignDelegation,
    #[error("the rater's account is not delegated and no authorization was supplied")]
    MissingAuthorization,
    #[error("the authorization is not signed by the rater")]
    AuthorizationNotByRater,
    #[error("the authorization points at {got}, not the expected delegate {want}")]
    AuthorizationWrongDelegate { got: Address, want: Address },
    #[error("the authorization is for chain {got}, not {want}")]
    AuthorizationWrongChain { got: u64, want: u64 },
    #[error("this relay authorisation has already been used")]
    NonceAlreadyUsed,
    #[error("the deadline has passed")]
    Expired,
    #[error("could not reach the chain to check the rater's account")]
    RpcUnavailable,
    #[error("the delegate at {0} has no code on this network")]
    DelegateNotDeployed(Address),
    #[error("the registry at {0} has no code on this network")]
    RegistryNotDeployed(Address),
    #[error("the delegate is pinned to registry {got}, but this network uses {want}")]
    DelegateWrongRegistry { got: Address, want: Address },
}

impl RelayError {
    /// Bounded token for logs and API responses. Never the raw error.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoDelegate(_) => "relay_no_delegate",
            Self::BadSignature => "relay_bad_signature",
            Self::ForeignDelegation => "relay_foreign_delegation",
            Self::MissingAuthorization => "relay_missing_authorization",
            Self::AuthorizationNotByRater => "relay_authorization_not_by_rater",
            Self::AuthorizationWrongDelegate { .. } => "relay_authorization_wrong_delegate",
            Self::AuthorizationWrongChain { .. } => "relay_authorization_wrong_chain",
            Self::NonceAlreadyUsed => "relay_nonce_already_used",
            Self::Expired => "relay_expired",
            Self::RpcUnavailable => "relay_rpc_unavailable",
            Self::DelegateNotDeployed(_) => "relay_delegate_not_deployed",
            Self::RegistryNotDeployed(_) => "relay_registry_not_deployed",
            Self::DelegateWrongRegistry { .. } => "relay_delegate_wrong_registry",
        }
    }
}

/// Build the registry calldata the rater is going to authorise.
#[allow(clippy::too_many_arguments)]
pub fn give_feedback_calldata(
    agent_id: u64,
    value: i128,
    value_decimals: u8,
    tag1: &str,
    tag2: &str,
    endpoint: &str,
    feedback_uri: &str,
    feedback_hash: FixedBytes<32>,
) -> Bytes {
    IReputationRegistry::giveFeedbackCall {
        agentId: U256::from(agent_id),
        value,
        valueDecimals: value_decimals,
        tag1: tag1.to_string(),
        tag2: tag2.to_string(),
        endpoint: endpoint.to_string(),
        feedbackURI: feedback_uri.to_string(),
        feedbackHash: feedback_hash,
    }
    .abi_encode()
    .into()
}

/// The digest the rater's key must sign.
///
/// Mirrors `FeedbackDelegate.relayDigest` exactly:
///
/// ```solidity
/// MessageHashUtils.toEthSignedMessageHash(
///     keccak256(abi.encode(block.chainid, address(this), REPUTATION_REGISTRY,
///                          keccak256(data), deadline, nonce)))
/// ```
///
/// Under EIP-7702 `address(this)` IS the rater's EOA, which is why the rater
/// address goes in here and not the delegate's: the digest binds the
/// authorisation to the account, the chain, the registry, the exact calldata and
/// a deadline, so it cannot be replayed across any of them.
///
/// Computed locally rather than read from `relayDigest()` on the delegate: that
/// view would bind `address(this)` to the DELEGATE's own address, not the
/// rater's, and would silently produce a digest nobody can satisfy.
pub fn relay_digest(
    chain_id: u64,
    rater: Address,
    registry: Address,
    data: &Bytes,
    deadline: u64,
    nonce: FixedBytes<32>,
) -> B256 {
    let mut buf = Vec::with_capacity(6 * 32);
    buf.extend_from_slice(&U256::from(chain_id).to_be_bytes::<32>());
    buf.extend_from_slice(&[0u8; 12]);
    buf.extend_from_slice(rater.as_slice());
    buf.extend_from_slice(&[0u8; 12]);
    buf.extend_from_slice(registry.as_slice());
    buf.extend_from_slice(keccak256(data).as_slice());
    buf.extend_from_slice(&U256::from(deadline).to_be_bytes::<32>());
    buf.extend_from_slice(nonce.as_slice());

    let inner = keccak256(&buf);
    // EIP-191 personal_sign envelope, the same one MessageHashUtils applies.
    let mut prefixed = Vec::with_capacity(28 + 32);
    prefixed.extend_from_slice(b"\x19Ethereum Signed Message:\n32");
    prefixed.extend_from_slice(inner.as_slice());
    keccak256(&prefixed)
}

/// Does `signature` over `digest` recover to the rater?
pub fn signature_authorises(digest: B256, signature: &Bytes, rater: Address) -> bool {
    let Ok(sig) = Signature::try_from(signature.as_ref()) else {
        return false;
    };
    match sig.recover_address_from_prehash(&digest) {
        Ok(recovered) => recovered == rater,
        Err(_) => false,
    }
}

/// Read the rater's account and classify its delegation.
pub async fn delegation_state<P: Provider>(
    rpc: &P,
    rater: Address,
    delegate: Address,
) -> Result<DelegationState, RelayError> {
    let code = rpc
        .get_code_at(rater)
        .await
        .map_err(|_| RelayError::RpcUnavailable)?;
    if code.is_empty() {
        return Ok(DelegationState::None);
    }
    // EIP-7702 designator: 0xef0100 || 20-byte delegate address.
    if code.len() == 23 && code[0] == 0xef && code[1] == 0x01 && code[2] == 0x00 {
        let current = Address::from_slice(&code[3..23]);
        return Ok(if current == delegate {
            DelegationState::Delegated
        } else {
            DelegationState::Foreign
        });
    }
    Ok(DelegationState::Foreign)
}

/// Refuse to use a delegate address that has no contract behind it, or one
/// pinned to a different registry than this network's.
///
/// Not paranoia: an `upto` proxy address with no code on any chain once produced
/// settles that reported success while moving nothing. An address in a table is
/// a claim; `eth_getCode` is evidence.
pub async fn assert_delegate_usable<P: Provider>(
    rpc: &P,
    delegate: Address,
    expected_registry: Address,
) -> Result<(), RelayError> {
    let code = rpc
        .get_code_at(delegate)
        .await
        .map_err(|_| RelayError::RpcUnavailable)?;
    if code.is_empty() {
        return Err(RelayError::DelegateNotDeployed(delegate));
    }
    let pinned = IFeedbackDelegate::new(delegate, rpc)
        .REPUTATION_REGISTRY()
        .call()
        .await
        .map_err(|_| RelayError::RpcUnavailable)?;
    if pinned != expected_registry {
        return Err(RelayError::DelegateWrongRegistry {
            got: pinned,
            want: expected_registry,
        });
    }

    // And the registry itself must have code. `relayFeedback` finishes with
    // `(bool ok, ) = REPUTATION_REGISTRY.call(data)`, and in the EVM a call to
    // an address with NO code SUCCEEDS -- it returns true and empty data. So a
    // delegate pinned to an address that was never deployed on this chain
    // reports a relayed rating, emits `FeedbackRelayed`, spends the nonce, and
    // rates nobody.
    //
    // Observed, not theorised: it happened on the first run of the end-to-end
    // rehearsal (2026-08-14), where a delegate pinned to the real testnet
    // registry ran against a local chain that had no such contract and returned
    // status 1. Same shape as the upto proxy whose address had no code anywhere
    // and produced fake-success settles.
    let registry_code = rpc
        .get_code_at(expected_registry)
        .await
        .map_err(|_| RelayError::RpcUnavailable)?;
    if registry_code.is_empty() {
        return Err(RelayError::RegistryNotDeployed(expected_registry));
    }
    Ok(())
}

/// Validate a rater-supplied 7702 authorisation before it goes into a
/// transaction we pay for.
pub fn accept_authorization(
    authorization: &SignedAuthorization,
    rater: Address,
    delegate: Address,
    chain_id: u64,
) -> Result<(), RelayError> {
    if *authorization.address() != delegate {
        return Err(RelayError::AuthorizationWrongDelegate {
            got: *authorization.address(),
            want: delegate,
        });
    }
    // Chain id 0 is the EIP-7702 wildcard, valid on every chain. Accepting it is
    // the spec's behaviour, but it is worth knowing that it is what the rater
    // signed: an authorisation valid everywhere is a broader grant than one
    // pinned to this chain.
    let auth_chain = authorization.chain_id();
    if *auth_chain != U256::ZERO && *auth_chain != U256::from(chain_id) {
        return Err(RelayError::AuthorizationWrongChain {
            got: auth_chain.to::<u64>(),
            want: chain_id,
        });
    }
    match recover_authority(authorization) {
        Some(authority) if authority == rater => Ok(()),
        _ => Err(RelayError::AuthorizationNotByRater),
    }
}

/// Who signed this authorisation?
///
/// Hand-rolled because `SignedAuthorization::recover_authority` sits behind
/// alloy's `k256` feature, which this build does not enable. The malleability
/// check is not optional: without rejecting the high half of the curve, the same
/// authorisation has a second valid encoding, and "already seen" checks keyed on
/// its bytes stop meaning anything.
fn recover_authority(authorization: &SignedAuthorization) -> Option<Address> {
    let signature = authorization.signature().ok()?;
    if signature.s() > alloy::eips::eip7702::constants::SECP256K1N_HALF {
        return None;
    }
    signature
        .recover_address_from_prehash(&authorization.signature_hash())
        .ok()
}

/// Assemble the signed authorisation from the raw parts a wallet produces.
pub fn signed_authorization(
    chain_id: U256,
    delegate: Address,
    nonce: u64,
    y_parity: u8,
    r: U256,
    s: U256,
) -> SignedAuthorization {
    SignedAuthorization::new_unchecked(
        Authorization {
            chain_id,
            address: delegate,
            nonce,
        },
        y_parity,
        r,
        s,
    )
}

/// Build the calldata sent TO THE RATER'S ADDRESS.
pub fn relay_feedback_calldata(
    data: &Bytes,
    deadline: u64,
    nonce: FixedBytes<32>,
    signature: &Bytes,
) -> Bytes {
    IFeedbackDelegate::relayFeedbackCall {
        data: data.clone(),
        deadline: U256::from(deadline),
        nonce,
        signature: signature.clone(),
    }
    .abi_encode()
    .into()
}

/// Has this relay nonce already been spent on the rater's account?
///
/// Read against the RATER's address, not the delegate's: under 7702 the storage
/// the delegate touches is the rater's own, so the nonce map lives there. Asking
/// the delegate would read a mapping that is always empty and would report every
/// spent nonce as fresh.
pub async fn nonce_already_used<P: Provider>(
    rpc: &P,
    rater: Address,
    nonce: FixedBytes<32>,
) -> Result<bool, RelayError> {
    IFeedbackDelegate::new(rater, rpc)
        .nonceUsed(nonce)
        .call()
        .await
        .map_err(|_| RelayError::RpcUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;

    const REGISTRY: Address =
        alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713");

    fn delegate() -> Address {
        delegate_address(&Network::BaseSepolia).unwrap()
    }

    /// The address EM deployed and we verified on-chain. Pinned so a typo in a
    /// later edit is a failing test rather than a transaction to nowhere.
    #[test]
    fn the_base_sepolia_delegate_is_the_verified_one() {
        assert_eq!(
            delegate().to_string().to_lowercase(),
            "0x3a68085499b62286468a35b7d9dfc237ef2d3768"
        );
    }

    /// The eight mainnet addresses Execution Market deployed, each read off its
    /// own chain on two independent RPCs before being written here (2026-08-23).
    /// Pinned so a typo in a later edit is a failing test rather than a type-4
    /// transaction sent to an address with no delegate behind it.
    #[test]
    fn the_mainnet_delegates_are_the_verified_ones() {
        let expected = [
            (Network::Base, "0x754206c4247317768bd86459e829a174d9c68ba4"),
            (
                Network::Ethereum,
                "0xbecea4673c0105af63d02688be6de6ca51d57dd9",
            ),
            (
                Network::Polygon,
                "0xf670c69bcbb2453fae5ec009c2b6dd934be46a7f",
            ),
            (
                Network::Arbitrum,
                "0x794c907fdfc71bfaf0b86d0e463bbd6e949a31ba",
            ),
            (Network::Bsc, "0x9551263b9b83b1a737d55fd5e67fb6d60e4ef787"),
            (
                Network::Optimism,
                "0x825e997f2f7ed5d3f59466cd754189fb19b62b82",
            ),
            (Network::Monad, "0x825e997f2f7ed5d3f59466cd754189fb19b62b82"),
            (Network::Celo, "0xe25cf9b9f5a3b5faa7628c751466df0166d96b59"),
        ];
        for (network, want) in expected {
            let got = feedback_delegate(&network)
                .unwrap_or_else(|| panic!("{network} lost its FeedbackDelegate"));
            assert_eq!(got.to_string().to_lowercase(), want, "{network}");
        }
    }

    /// Avalanche must NEVER get an entry here, and this test is the guard.
    ///
    /// Its C-Chain rejects the transaction type itself -- `-32000 transaction
    /// type not supported`, an explicit refusal from the node, not an absence of
    /// traffic -- and no C-Chain upgrade has shipped EIP-7702. Reputation for
    /// tasks paid on Avalanche is routed to another chain by Execution Market;
    /// the payment stays on Avalanche. An address added here would build a
    /// transaction every node on the network refuses to accept.
    ///
    /// Scroll and SKALE Base serve ERC-8004 but have no delegate deployed
    /// (SKALE's EVM predates Shanghai, so 7702 cannot land there at all).
    #[test]
    fn the_chains_without_a_delegate_claim_none() {
        for network in [
            Network::Avalanche,
            Network::AvalancheFuji,
            Network::Scroll,
            Network::SkaleBase,
            Network::EthereumSepolia,
            Network::PolygonAmoy,
            Network::ArbitrumSepolia,
            Network::OptimismSepolia,
            Network::CeloSepolia,
        ] {
            assert!(
                feedback_delegate(&network).is_none(),
                "{network} must not claim a delegate that was never deployed"
            );
        }
    }

    /// Optimism and Monad genuinely share an address: same deployer, same salt,
    /// same init code through CREATE2 lands on the same address on both chains.
    /// Asserted so a future reader does not "fix" what looks like a copy-paste
    /// slip. The per-chain `assert_delegate_usable` check still runs at request
    /// time on each of them.
    #[test]
    fn optimism_and_monad_share_an_address_on_purpose() {
        assert_eq!(
            feedback_delegate(&Network::Optimism),
            feedback_delegate(&Network::Monad)
        );
    }

    /// A delegate is only ever served where the facilitator also serves the
    /// ERC-8004 registries: without contracts there is no registry address to
    /// pin the delegate against, and `relay_context` would fail after we had
    /// already promised the caller a delegate.
    #[test]
    fn every_delegate_network_has_erc8004_contracts() {
        for network in Network::variants() {
            if feedback_delegate(network).is_some() {
                assert!(
                    crate::erc8004::get_contracts(network).is_some(),
                    "{network} has a delegate but no ERC-8004 contracts"
                );
            }
        }
    }

    /// The digest is pinned against the CONTRACT, not against ourselves.
    ///
    /// Measured on 2026-08-14: anvil --hardfork prague (chain 31337), Execution
    /// Market's real `FeedbackDelegate` deployed with the testnet registry, a
    /// genuine type-4 delegation of the rater's EOA to it, and then
    /// `relayDigest(data, deadline, nonce)` called ON THE DELEGATED ACCOUNT --
    /// which is the only way to get the contract to compute it with
    /// `address(this)` equal to the rater, exactly as it will at relay time.
    ///
    /// Pinning it against a value this module produced would prove only that
    /// the module agrees with itself. Three fabricated SHA-256 variants of the
    /// SEAL hash passed CI for months that way.
    #[test]
    fn the_digest_matches_the_deployed_contract() {
        // giveFeedback(42, 87, 0, "quality", "api", "https://agent.example",
        //              "https://example.com/f.json", 0x00..00)
        let data = give_feedback_calldata(
            42,
            87,
            0,
            "quality",
            "api",
            "https://agent.example",
            "https://example.com/f.json",
            FixedBytes::<32>::ZERO,
        );
        let rater = alloy::primitives::address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
        let digest = relay_digest(
            31337,
            rater,
            REGISTRY,
            &data,
            1_786_400_000,
            FixedBytes::<32>::from([0x22; 32]),
        );
        assert_eq!(
            format!("{digest:#x}"),
            "0xbf7b1043399af22fe8098d5a9cc7f928c8e27c8673ceae8c863fd68ec06f1a36",
            "the digest no longer matches what FeedbackDelegate computes on-chain; \
             a rater signing this would be rejected with NotAccountOwner"
        );
    }

    /// The digest is what the rater signs, so every field in it has to change
    /// it. If any of these collapsed, an authorisation for one rating would
    /// authorise another.
    #[test]
    fn every_field_of_the_digest_binds() {
        let rater = Address::from([0x11; 20]);
        let data: Bytes = vec![1u8, 2, 3].into();
        let nonce = FixedBytes::<32>::from([0x22; 32]);
        let base = relay_digest(84532, rater, REGISTRY, &data, 1000, nonce);

        assert_ne!(base, relay_digest(1, rater, REGISTRY, &data, 1000, nonce));
        assert_ne!(
            base,
            relay_digest(
                84532,
                Address::from([0x99; 20]),
                REGISTRY,
                &data,
                1000,
                nonce
            )
        );
        assert_ne!(
            base,
            relay_digest(84532, rater, Address::from([0x99; 20]), &data, 1000, nonce)
        );
        assert_ne!(
            base,
            relay_digest(84532, rater, REGISTRY, &vec![9u8].into(), 1000, nonce)
        );
        assert_ne!(
            base,
            relay_digest(84532, rater, REGISTRY, &data, 1001, nonce)
        );
        assert_ne!(
            base,
            relay_digest(
                84532,
                rater,
                REGISTRY,
                &data,
                1000,
                FixedBytes::<32>::from([0x33; 32])
            )
        );
    }

    #[test]
    fn a_rater_signature_over_the_digest_is_accepted_and_a_strangers_is_not() {
        let signer = PrivateKeySigner::random();
        let stranger = PrivateKeySigner::random();
        let data: Bytes = vec![1u8, 2, 3].into();
        let nonce = FixedBytes::<32>::from([0x22; 32]);
        let digest = relay_digest(84532, signer.address(), REGISTRY, &data, 1000, nonce);

        let sig: Bytes = signer
            .sign_hash_sync(&digest)
            .unwrap()
            .as_bytes()
            .to_vec()
            .into();
        assert!(signature_authorises(digest, &sig, signer.address()));
        // Same signature, different claimed author.
        assert!(!signature_authorises(digest, &sig, stranger.address()));

        let forged: Bytes = stranger
            .sign_hash_sync(&digest)
            .unwrap()
            .as_bytes()
            .to_vec()
            .into();
        assert!(!signature_authorises(digest, &forged, signer.address()));
    }

    #[test]
    fn garbage_is_not_a_signature() {
        let digest = B256::from([0x44; 32]);
        assert!(!signature_authorises(digest, &vec![].into(), Address::ZERO));
        assert!(!signature_authorises(
            digest,
            &vec![0u8; 10].into(),
            Address::ZERO
        ));
        assert!(!signature_authorises(
            digest,
            &vec![0u8; 65].into(),
            Address::ZERO
        ));
    }

    fn auth_for(signer: &PrivateKeySigner, chain_id: u64, address: Address) -> SignedAuthorization {
        let auth = Authorization {
            chain_id: U256::from(chain_id),
            address,
            nonce: 7,
        };
        let sig = signer.sign_hash_sync(&auth.signature_hash()).unwrap();
        auth.into_signed(sig)
    }

    #[test]
    fn an_authorization_by_the_rater_for_our_delegate_is_accepted() {
        let signer = PrivateKeySigner::random();
        let auth = auth_for(&signer, 84532, delegate());
        assert!(accept_authorization(&auth, signer.address(), delegate(), 84532).is_ok());
    }

    /// The attack: an authorisation delegating the rater's account to a contract
    /// of the attacker's choosing, smuggled in for us to pay for. The delegate
    /// address is ours to decide, never theirs.
    #[test]
    fn an_authorization_for_another_delegate_is_refused() {
        let signer = PrivateKeySigner::random();
        let hostile = Address::from([0xbe; 20]);
        let auth = auth_for(&signer, 84532, hostile);
        assert!(matches!(
            accept_authorization(&auth, signer.address(), delegate(), 84532),
            Err(RelayError::AuthorizationWrongDelegate { .. })
        ));
    }

    #[test]
    fn an_authorization_signed_by_somebody_else_is_refused() {
        let signer = PrivateKeySigner::random();
        let stranger = PrivateKeySigner::random();
        let auth = auth_for(&stranger, 84532, delegate());
        assert!(matches!(
            accept_authorization(&auth, signer.address(), delegate(), 84532),
            Err(RelayError::AuthorizationNotByRater)
        ));
    }

    #[test]
    fn an_authorization_for_another_chain_is_refused() {
        let signer = PrivateKeySigner::random();
        let auth = auth_for(&signer, 1, delegate());
        assert!(matches!(
            accept_authorization(&auth, signer.address(), delegate(), 84532),
            Err(RelayError::AuthorizationWrongChain { .. })
        ));
    }

    /// Chain id 0 is EIP-7702's wildcard and is valid everywhere. Accepted, but
    /// pinned by a test so nobody later "fixes" it into a rejection and breaks
    /// every wallet that emits the wildcard.
    #[test]
    fn the_wildcard_chain_id_is_accepted() {
        let signer = PrivateKeySigner::random();
        let auth = auth_for(&signer, 0, delegate());
        assert!(accept_authorization(&auth, signer.address(), delegate(), 84532).is_ok());
    }

    /// The calldata the rater signs must be the calldata we relay. This pins the
    /// selector so a change in the ABI cannot silently start authorising a
    /// different call.
    #[test]
    fn the_relayed_calldata_is_give_feedback() {
        let data = give_feedback_calldata(
            42,
            87,
            0,
            "quality",
            "api",
            "https://agent.example",
            "https://example.com/f.json",
            FixedBytes::<32>::ZERO,
        );
        // giveFeedback(uint256,int128,uint8,string,string,string,string,bytes32)
        assert_eq!(&data[..4], &[0x3c, 0x03, 0x6a, 0x7e]);
    }

    #[test]
    fn the_outer_call_is_relay_feedback() {
        let data: Bytes = vec![1u8, 2, 3].into();
        let calldata = relay_feedback_calldata(
            &data,
            1000,
            FixedBytes::<32>::from([0x22; 32]),
            &vec![0u8; 65].into(),
        );
        let expected =
            &alloy::primitives::keccak256(b"relayFeedback(bytes,uint256,bytes32,bytes)")[..4];
        assert_eq!(&calldata[..4], expected);
    }

    #[test]
    fn the_deadline_window_is_short_by_default_and_configurable() {
        std::env::remove_var(ENV_RELAY_DEADLINE_SECS);
        assert_eq!(relay_deadline_secs(), 900);
        std::env::set_var(ENV_RELAY_DEADLINE_SECS, "60");
        assert_eq!(relay_deadline_secs(), 60);
        // Zero or garbage must not mean "never expires".
        std::env::set_var(ENV_RELAY_DEADLINE_SECS, "0");
        assert_eq!(relay_deadline_secs(), 900);
        std::env::set_var(ENV_RELAY_DEADLINE_SECS, "forever");
        assert_eq!(relay_deadline_secs(), 900);
        std::env::remove_var(ENV_RELAY_DEADLINE_SECS);
    }

    #[test]
    fn every_relay_error_has_a_bounded_token() {
        let all = [
            RelayError::NoDelegate(Network::Base),
            RelayError::BadSignature,
            RelayError::ForeignDelegation,
            RelayError::MissingAuthorization,
            RelayError::AuthorizationNotByRater,
            RelayError::AuthorizationWrongDelegate {
                got: Address::ZERO,
                want: Address::ZERO,
            },
            RelayError::AuthorizationWrongChain { got: 1, want: 2 },
            RelayError::NonceAlreadyUsed,
            RelayError::Expired,
            RelayError::RpcUnavailable,
            RelayError::DelegateNotDeployed(Address::ZERO),
            RelayError::RegistryNotDeployed(Address::ZERO),
            RelayError::DelegateWrongRegistry {
                got: Address::ZERO,
                want: Address::ZERO,
            },
        ];
        let mut seen = std::collections::HashSet::new();
        for e in &all {
            let token = e.as_str();
            assert!(seen.insert(token), "duplicate token: {token}");
            // The token reaches clients and logs; it must carry no address.
            assert!(!token.contains("0x"));
        }
        assert_eq!(seen.len(), 13);
    }
}
