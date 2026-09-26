//! Retiring an ERC-8004 identity the facilitator holds.
//!
//! Until 2.44.0 `POST /register` without a `recipient` minted the identity to
//! the facilitator's own wallet and left it there, with whatever `agentUri` the
//! caller chose. Some of those URIs point at hosts nobody should be sent to,
//! and the registry shows them as ours.
//!
//! Retiring points the identity's `agentURI` at [`RETIRED_URI`], a document the
//! facilitator serves saying the identity represents no agent. Two things it
//! deliberately is not:
//!
//! - **Not a transfer to a dead address.** That would leave the old URI on the
//!   identity forever, with nobody able to change it.
//! - **Not a transaction signed by hand with the hot key.** The running service
//!   allocates the wallet's nonces (`PendingNonceManager`, `chain::evm`); a
//!   transaction from outside it takes a nonce the service believes is free, and
//!   the service's next write collides. This runs INSIDE the service, through
//!   the same provider as every other ERC-8004 write, behind the writer lease.
//!
//! It is exposed as an admin maintenance call
//! (`POST /erc8004/admin/retire-identity`, see `handlers.rs`), with a dry run
//! that reports what it would do and an idempotent real run.

use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use serde::Serialize;

use super::abi::IIdentityRegistry;
use crate::chain::evm::{EvmProvider, MetaEvmProvider};
use crate::network::Network;

/// Where a retired identity's `agentURI` points. Fixed rather than configured:
/// "already retired" is decided by comparing against it, so it must be the same
/// string on every deploy, and it is what the chain keeps.
pub const RETIRED_URI: &str = "https://facilitator.ultravioletadao.xyz/erc8004/retired";

/// The document served at [`RETIRED_URI`], shaped as an ERC-8004 registration
/// file so indexers that fetch `agentURI` read it as one.
pub fn retired_document() -> serde_json::Value {
    serde_json::json!({
        "type": "https://eips.ethereum.org/EIPS/eip-8004#registration-v1",
        "name": "Retired identity",
        "description": "This ERC-8004 identity is held by the Ultravioleta DAO x402 facilitator \
                        and has been retired: the agentURI it was minted with did not meet the \
                        facilitator's registration rules and was withdrawn. It represents no \
                        agent and offers no service. Do not contact or trust anything on the \
                        strength of this identity.",
        "active": false,
        "endpoints": [],
        "supportedTrust": []
    })
}

/// What a retire call did, or would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetireStatus {
    /// Dry run: the identity is ours and would be retired.
    WouldRetire,
    /// Its `agentURI` already is [`RETIRED_URI`]; nothing was sent.
    AlreadyRetired,
    /// `setAgentURI` confirmed on chain.
    Retired,
}

/// The outcome of a retire call that got an answer from the chain.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Retirement {
    pub status: RetireStatus,
    pub owner: Address,
    /// The `agentURI` before this call.
    pub previous_uri: String,
    /// The `POST /register` rules the previous URI breaks, for the operator
    /// deciding whether to retire it (`agent_uri::violations`).
    pub previous_uri_violations: Vec<&'static str>,
    pub retired_uri: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<B256>,
}

/// Why a retire call did not complete.
#[derive(Debug)]
pub enum RetireError {
    /// The identity belongs to someone else; only its owner can change it.
    NotHeld { owner: Address },
    /// A read the decision depends on failed. Nothing was sent.
    Unreadable(String),
    /// Refused at estimation, or not accepted by the node. Nothing landed.
    NotSent(String),
    /// Sent, but the receipt did not come back in time. It may still land;
    /// repeating the call is safe (it answers `already_retired` once it has).
    Unconfirmed { transaction: B256, error: String },
    /// Mined and reverted.
    Reverted { transaction: B256 },
}

/// Retire `agent_id` in `registry` if the facilitator holds it.
///
/// Reads the owner and the current URI first, so a dry run and a repeated call
/// cost no gas and an identity that is not ours is never touched.
pub async fn retire_identity(
    provider: &EvmProvider,
    registry: Address,
    agent_id: U256,
    network: Network,
    dry_run: bool,
    receipt_timeout: Duration,
) -> Result<Retirement, RetireError> {
    let identity_registry = IIdentityRegistry::new(registry, provider.inner().clone());

    let owner = identity_registry
        .ownerOf(agent_id)
        .call()
        .await
        .map_err(|e| RetireError::Unreadable(format!("ownerOf({agent_id}) failed: {e}")))?;
    if !provider.controls_signer(owner) {
        return Err(RetireError::NotHeld { owner });
    }
    let previous_uri = identity_registry
        .tokenURI(agent_id)
        .call()
        .await
        .map_err(|e| RetireError::Unreadable(format!("tokenURI({agent_id}) failed: {e}")))?;
    let retirement = |status, transaction| Retirement {
        status,
        owner,
        previous_uri_violations: super::agent_uri::violations(&previous_uri)
            .into_iter()
            .map(super::agent_uri::Violation::code)
            .collect(),
        previous_uri: previous_uri.clone(),
        retired_uri: RETIRED_URI,
        transaction,
    };
    if previous_uri == RETIRED_URI {
        return Ok(retirement(RetireStatus::AlreadyRetired, None));
    }
    if dry_run {
        return Ok(retirement(RetireStatus::WouldRetire, None));
    }

    // From the signer that owns it, through the provider's own nonce manager.
    let call = identity_registry
        .setAgentURI(agent_id, RETIRED_URI.to_string())
        .from(owner);
    let sent = if provider.is_eip1559() {
        crate::chain::evm::send_call_estimated(call, network).await
    } else {
        // Legacy chains (SKALE) need an explicit gasPrice, as in the mint.
        let gas_price = provider
            .inner()
            .get_gas_price()
            .await
            .map_err(|e| RetireError::Unreadable(format!("gas price: {e}")))?;
        crate::chain::evm::send_call_estimated(call.gas_price(gas_price), network).await
    };
    let pending = sent.map_err(|e| RetireError::NotSent(e.to_string()))?;
    let transaction = *pending.tx_hash();
    let receipt = pending
        .with_timeout(Some(receipt_timeout))
        .get_receipt()
        .await
        .map_err(|e| RetireError::Unconfirmed {
            transaction,
            error: e.to_string(),
        })?;
    if !receipt.status() {
        return Err(RetireError::Reverted { transaction });
    }
    Ok(retirement(RetireStatus::Retired, Some(transaction)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payment_operator::test_rpc::{self, CallAnswer, MockNode};
    use alloy::sol_types::{SolCall, SolValue};

    const AGENT: u64 = 95_531;
    /// A documentation-range host, never the real one.
    const BAD_URI: &str = "http://198-51-100-7.sslip.io/agent.json";

    fn registry() -> Address {
        crate::erc8004::get_contracts(&Network::Base)
            .unwrap()
            .identity_registry
    }

    /// A node whose registry says `owner` holds the identity with `uri`.
    async fn node(owner: Address, uri: &str) -> (MockNode, EvmProvider) {
        let node = MockNode::start(Network::Base).await;
        node.on_call(
            registry(),
            IIdentityRegistry::ownerOfCall::SELECTOR,
            CallAnswer::Return(owner.abi_encode()),
        );
        node.on_call(
            registry(),
            IIdentityRegistry::tokenURICall::SELECTOR,
            CallAnswer::Return((uri.to_string(),).abi_encode_params()),
        );
        let provider = test_rpc::provider(Network::Base, &node, true).await;
        (node, provider)
    }

    async fn retire(provider: &EvmProvider, dry_run: bool) -> Result<Retirement, RetireError> {
        retire_identity(
            provider,
            registry(),
            U256::from(AGENT),
            Network::Base,
            dry_run,
            Duration::from_secs(5),
        )
        .await
    }

    #[tokio::test]
    async fn a_dry_run_reports_and_sends_nothing() {
        let signer = test_rpc::fixed_wallet().default_signer().address();
        let (node, provider) = node(signer, BAD_URI).await;
        let r = retire(&provider, true).await.unwrap();
        assert_eq!(r.status, RetireStatus::WouldRetire);
        assert_eq!(r.previous_uri, BAD_URI);
        assert_eq!(
            r.previous_uri_violations,
            vec!["agent_uri_scheme", "agent_uri_embedded_ip"]
        );
        assert!(r.transaction.is_none());
        assert!(node.sent().is_empty(), "a dry run sent a transaction");
    }

    /// The real run sends exactly one `setAgentURI(agentId, RETIRED_URI)` to
    /// the registry, from the wallet that owns the identity.
    #[tokio::test]
    async fn retiring_sends_one_set_agent_uri_from_the_owner() {
        let signer = test_rpc::fixed_wallet().default_signer().address();
        let (node, provider) = node(signer, BAD_URI).await;
        let r = retire(&provider, false).await.unwrap();
        assert_eq!(r.status, RetireStatus::Retired);
        assert!(r.transaction.is_some());

        let sent = node.sent();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].to, Some(registry()));
        assert_eq!(sent[0].from, signer);
        let call = IIdentityRegistry::setAgentURICall::abi_decode(&sent[0].input).unwrap();
        assert_eq!(call.agentId, U256::from(AGENT));
        assert_eq!(call.newURI, RETIRED_URI);
    }

    /// Identities held by the facilitator's second signer are retired from it,
    /// not from the default one (which the registry would reject).
    #[tokio::test]
    async fn an_identity_on_another_of_our_signers_is_retired_from_that_signer() {
        let wallet = test_rpc::fixed_wallet();
        let default = wallet.default_signer().address();
        let other =
            alloy::network::NetworkWallet::<alloy::network::Ethereum>::signer_addresses(&wallet)
                .find(|a| *a != default)
                .expect("the fixed wallet has two signers");
        let (node, provider) = node(other, BAD_URI).await;
        retire(&provider, false).await.unwrap();
        assert_eq!(node.sent()[0].from, other);
    }

    /// A mined revert is a failure with its transaction, never "retired".
    #[tokio::test]
    async fn a_reverted_retirement_is_not_reported_as_done() {
        let signer = test_rpc::fixed_wallet().default_signer().address();
        let (node, provider) = node(signer, BAD_URI).await;
        node.revert_receipts(true);
        match retire(&provider, false).await {
            Err(RetireError::Reverted { transaction }) => assert_ne!(transaction, B256::ZERO),
            other => panic!("expected Reverted, got {other:?}"),
        }
        assert_eq!(node.sent().len(), 1);
    }

    /// Repeating the call after it landed is free and says so.
    #[tokio::test]
    async fn an_already_retired_identity_is_left_alone() {
        let signer = test_rpc::fixed_wallet().default_signer().address();
        let (node, provider) = node(signer, RETIRED_URI).await;
        let r = retire(&provider, false).await.unwrap();
        assert_eq!(r.status, RetireStatus::AlreadyRetired);
        assert!(r.previous_uri_violations.is_empty());
        assert!(node.sent().is_empty());
    }

    /// Somebody else's identity is never touched, dry run or not.
    #[tokio::test]
    async fn an_identity_we_do_not_hold_is_refused() {
        let stranger = Address::repeat_byte(0x77);
        let (node, provider) = node(stranger, BAD_URI).await;
        for dry_run in [true, false] {
            match retire(&provider, dry_run).await {
                Err(RetireError::NotHeld { owner }) => assert_eq!(owner, stranger),
                other => panic!("expected NotHeld, got {other:?}"),
            }
        }
        assert!(node.sent().is_empty());
    }

    /// No owner verdict, no transaction.
    #[tokio::test]
    async fn an_unreadable_owner_sends_nothing() {
        let signer = test_rpc::fixed_wallet().default_signer().address();
        let (node, provider) = node(signer, BAD_URI).await;
        node.fail_reads(true);
        assert!(matches!(
            retire(&provider, false).await,
            Err(RetireError::Unreadable(_))
        ));
        assert!(node.sent().is_empty());
    }

    /// What the chain will keep must itself pass the rules `/register` applies.
    #[test]
    fn the_retired_uri_passes_the_register_rules() {
        assert_eq!(crate::erc8004::agent_uri::check(RETIRED_URI), Ok(()));
        let doc = retired_document();
        assert_eq!(doc["active"], false);
        assert_eq!(
            doc["type"],
            "https://eips.ethereum.org/EIPS/eip-8004#registration-v1"
        );
    }
}
