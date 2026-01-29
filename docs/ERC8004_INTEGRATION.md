# ERC-8004 Reputation Extension Integration Plan

## Executive Summary

**Objective**: Integrate ERC-8004 (Trustless Agents) reputation extension into x402-rs facilitator.

**Deadline**: Thursday, January 29, 2025 @ 9:00 AM (ERC-8004 mainnet launch)

**Current Date**: Monday, January 26, 2025 @ 9:30 PM

**Time Available**: ~60 hours

---

## Table of Contents

1. [What is ERC-8004?](#1-what-is-erc-8004)
2. [Contract Addresses](#2-contract-addresses)
3. [Extension Specification](#3-extension-specification-8004-reputation)
4. [Architecture Changes](#4-architecture-changes)
5. [Implementation Phases](#5-implementation-phases)
6. [Detailed Tasks](#6-detailed-tasks)
7. [Solidity ABIs](#7-solidity-abis)
8. [Rust Type Definitions](#8-rust-type-definitions)
9. [Testing Plan](#9-testing-plan)
10. [Deployment Checklist](#10-deployment-checklist)
11. [References](#11-references)

---

## 1. What is ERC-8004?

ERC-8004 is an Ethereum standard for **Trustless AI Agents** that provides three on-chain registries:

| Registry | Purpose | Contract Type |
|----------|---------|---------------|
| **Identity Registry** | Agent registration as NFT (ERC-721) | Stores agent metadata, wallet addresses |
| **Reputation Registry** | Feedback system for agents | Stores scores, tags, proof of interaction |
| **Validation Registry** | Independent work verification | Third-party validation requests/responses |

### Why Integrate with x402?

The `8004-reputation` extension links **payment settlements to reputation signals**:

```
Payment Settled → ProofOfPayment Generated → Feedback Submitted → On-chain Reputation Updated
```

This creates:
- **Verifiable reputation** tied to actual payments (no fake reviews)
- **Bidirectional trust** - clients rate servers, servers rate clients
- **Sybil resistance** - feedback requires proof of payment transaction

---

## 2. Contract Addresses

### Deterministic Addresses (Same on ALL Networks)

```
Identity Registry:   0x7177a6867296406881E20d6647232314736Dd09A
Reputation Registry: 0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322
Validation Registry: 0x662b40A526cb4017d947e71eAF6753BF3eeE66d8
```

### Testnet Deployment Status (As of Jan 26, 2025)

| Network | Chain ID | Status | RPC |
|---------|----------|--------|-----|
| Ethereum Sepolia | 11155111 | ✅ Live | `https://rpc.sepolia.org` |
| Base Sepolia | 84532 | ✅ Live | `https://sepolia.base.org` |
| Optimism Sepolia | 11155420 | ✅ Live | `https://sepolia.optimism.io` |
| Mode Testnet | 919 | ✅ Live | - |
| 0G Testnet | 16602 | ✅ Live | - |

### Mainnet Deployment (Expected Jan 30, 2025 @ 9:00 AM)

**CRITICAL**: Mainnet addresses will be the SAME as testnet (deterministic deployment via CREATE2).

| Network | Chain ID | Our Support |
|---------|----------|-------------|
| Base Mainnet | 8453 | ✅ Already supported |
| Ethereum Mainnet | 1 | ✅ Already supported |
| Optimism Mainnet | 10 | ✅ Already supported |
| Arbitrum Mainnet | 42161 | ✅ Already supported |

---

## 3. Extension Specification: `8004-reputation`

### 3.1 Extension Identifier

```
8004-reputation
```

### 3.2 PaymentRequired Flow (Server → Client)

Server announces its agent identity when requesting payment:

```json
{
  "paymentRequirements": {
    "scheme": "exact",
    "network": "eip155:8453",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "payTo": "0xServerWallet...",
    "maxAmountRequired": "1000000",
    "extra": {
      "8004-reputation": {
        "agentRegistry": "eip155:8453:0x7177a6867296406881E20d6647232314736Dd09A",
        "agentId": "42",
        "reputationRegistry": "eip155:8453:0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322",
        "endpoint": "https://agent.example.com/a2a"
      }
    }
  }
}
```

### 3.3 PaymentPayload Flow (Client → Facilitator)

Client echoes server identity and optionally provides their own:

```json
{
  "x402Version": 1,
  "payload": {
    "signature": "0x...",
    "authorization": { ... }
  },
  "extra": {
    "8004-reputation": {
      "serverIdentity": {
        "agentRegistry": "eip155:8453:0x7177a6867296406881E20d6647232314736Dd09A",
        "agentId": "42",
        "reputationRegistry": "eip155:8453:0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322"
      },
      "clientIdentity": {
        "agentRegistry": "eip155:8453:0x7177a6867296406881E20d6647232314736Dd09A",
        "agentId": "99",
        "reputationRegistry": "eip155:8453:0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322"
      }
    }
  }
}
```

### 3.4 Settlement Response (Facilitator → Client)

Facilitator returns proof of payment for feedback submission:

```json
{
  "success": true,
  "transaction": "0xabc123...",
  "network": "eip155:8453",
  "proofOfPayment": {
    "txHash": "0xabc123...",
    "network": "eip155:8453",
    "blockNumber": 12345678,
    "blockHash": "0xdef456...",
    "timestamp": 1706500000,
    "from": "0xClientAddress...",
    "to": "0xServerAddress...",
    "amount": "1000000",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  }
}
```

### 3.5 Feedback Submission (Client → Facilitator)

New endpoint to submit feedback on-chain:

```
POST /feedback
```

```json
{
  "agentId": "42",
  "reputationRegistry": "eip155:8453:0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322",
  "proofOfPayment": {
    "txHash": "0xabc123...",
    "network": "eip155:8453",
    "blockNumber": 12345678
  },
  "feedback": {
    "score": 100,
    "tag1": "x402-resource-delivered",
    "tag2": "exact-evm",
    "feedbackUri": "ipfs://Qm...",
    "feedbackHash": "0x..."
  }
}
```

Response:

```json
{
  "success": true,
  "feedbackTx": "0xfeedback123...",
  "feedbackIndex": 5
}
```

### 3.6 Standardized Tags

#### Client → Server (tag1)

| Tag | Meaning |
|-----|---------|
| `x402-resource-delivered` | Resource delivered successfully |
| `x402-delivery-failed` | Resource not delivered |
| `x402-delivery-timeout` | Delivery timed out |
| `x402-quality-issue` | Delivered but quality problems |

#### Server → Client (tag1)

| Tag | Meaning |
|-----|---------|
| `x402-good-payer` | Payment successful |
| `x402-payment-failed` | Payment failed |
| `x402-insufficient-funds` | Insufficient balance |
| `x402-invalid-signature` | Bad signature |

#### Network Identifier (tag2)

| Tag | Meaning |
|-----|---------|
| `exact-evm` | EVM network with exact scheme |
| `exact-svm` | Solana with exact scheme |

---

## 4. Architecture Changes

### 4.1 New Files to Create

```
src/
├── erc8004/
│   ├── mod.rs           # Module exports
│   ├── types.rs         # AgentIdentity, ProofOfPayment, FeedbackParams
│   ├── abi.rs           # Contract ABIs (Identity, Reputation)
│   ├── client.rs        # On-chain interaction client
│   └── extension.rs     # Extension parsing from PaymentRequirements
├── handlers.rs          # MODIFY: Add /feedback endpoint
├── types.rs             # MODIFY: Add extension types to PaymentPayload
└── chain/
    └── evm.rs           # MODIFY: Return ProofOfPayment from settle
```

### 4.2 Modified Files

| File | Changes |
|------|---------|
| `src/handlers.rs` | Add `post_feedback()` handler, modify `post_settle()` response |
| `src/types.rs` | Add `ProofOfPayment` to `SettleResponse` |
| `src/chain/evm.rs` | Capture block info after settlement for ProofOfPayment |
| `src/main.rs` | Register `/feedback` route |
| `Cargo.toml` | No new dependencies (use existing ethers/alloy) |

### 4.3 Data Flow Diagram

```
┌────────────────────────────────────────────────────────────────────────────┐
│                           ERC-8004 Integration Flow                        │
└────────────────────────────────────────────────────────────────────────────┘

1. PAYMENT REQUIRED (Server announces identity)
   ┌─────────┐                                              ┌─────────┐
   │ Server  │ ─── 402 + PaymentRequired + AgentIdentity ──▶│ Client  │
   └─────────┘                                              └────┬────┘
                                                                 │
2. PAYMENT PAYLOAD (Client includes identities)                  │
   ┌─────────────┐                                               │
   │ Client      │◀──────────────────────────────────────────────┘
   │ (prepares   │
   │  payload)   │
   └──────┬──────┘
          │
3. VERIFY (Optional - validates structure)
          │
          ▼
   ┌─────────────┐
   │ Facilitator │  POST /verify
   │             │  - Validate signature
   │             │  - Check agent identity (optional)
   └──────┬──────┘
          │
4. SETTLE (Execute payment, return ProofOfPayment)
          │
          ▼
   ┌─────────────┐
   │ Facilitator │  POST /settle
   │             │  - Submit EIP-3009 transferWithAuthorization
   │             │  - Wait for tx receipt
   │             │  - Build ProofOfPayment from receipt
   │             │  - Return to client
   └──────┬──────┘
          │
          ├──────────────────────────────────────────┐
          │                                          │
          ▼                                          ▼
   ┌─────────────┐                            ┌─────────────┐
   │   Client    │                            │   Server    │
   │ (has proof) │                            │ (has proof) │
   └──────┬──────┘                            └──────┬──────┘
          │                                          │
5. FEEDBACK (Submit reputation on-chain)             │
          │                                          │
          ▼                                          ▼
   ┌─────────────┐                            ┌─────────────┐
   │ Facilitator │  POST /feedback            │ Facilitator │  POST /feedback
   │             │  - Verify ProofOfPayment   │             │  (if client shared identity)
   │             │  - Call giveFeedback()     │             │
   │             │  - Return feedbackIndex    │             │
   └──────┬──────┘                            └─────────────┘
          │
          ▼
   ┌─────────────────────┐
   │ Reputation Registry │
   │ (on-chain)          │
   │ - NewFeedback event │
   │ - Score recorded    │
   └─────────────────────┘
```

---

## 5. Implementation Phases

### Phase 1: Core Types & ProofOfPayment (PRIORITY - Day 1)
**Estimated effort**: Medium
**Dependencies**: None

- [ ] Create `src/erc8004/mod.rs` module structure
- [ ] Create `src/erc8004/types.rs` with all type definitions
- [ ] Modify `src/chain/evm.rs` to return `ProofOfPayment`
- [ ] Modify `src/types.rs` to include `ProofOfPayment` in `SettleResponse`
- [ ] Modify `src/handlers.rs` `post_settle()` to return enriched response
- [ ] Test on Base Sepolia

### Phase 2: Extension Parsing (Day 1-2)
**Estimated effort**: Low
**Dependencies**: Phase 1

- [ ] Create `src/erc8004/extension.rs` for parsing `extra` field
- [ ] Add `AgentReputationInfo` extraction from `PaymentRequirements.extra`
- [ ] Add `AgentReputationInfo` extraction from `PaymentPayload.extra`
- [ ] Add optional validation: `payTo` == `getAgentWallet(agentId)`

### Phase 3: Feedback Endpoint (Day 2)
**Estimated effort**: High
**Dependencies**: Phase 1, Phase 2

- [ ] Create `src/erc8004/abi.rs` with Reputation Registry ABI
- [ ] Create `src/erc8004/client.rs` with `giveFeedback()` call
- [ ] Add `POST /feedback` endpoint in `src/handlers.rs`
- [ ] Register route in `src/main.rs`
- [ ] Handle gas estimation and tx submission
- [ ] Test on Base Sepolia

### Phase 4: Identity Verification (Day 2-3, Optional)
**Estimated effort**: Medium
**Dependencies**: Phase 2

- [ ] Add Identity Registry ABI
- [ ] Add `getAgentWallet()` call in client
- [ ] Add pre-settlement verification in `/settle`
- [ ] Log warning if `payTo` != registered `agentWallet`

### Phase 5: Mainnet Deployment (Day 3 - Thursday Morning)
**Estimated effort**: Low
**Dependencies**: Phase 1-3, Mainnet contracts live

- [ ] Verify mainnet contract addresses match testnet (deterministic)
- [ ] Update any mainnet-specific RPC configurations
- [ ] Deploy updated facilitator to ECS
- [ ] Run mainnet integration test
- [ ] Monitor first feedback submissions

---

## 6. Detailed Tasks

### Task 1: Create ERC-8004 Module Structure

**File**: `src/erc8004/mod.rs`

```rust
//! ERC-8004 Trustless Agents integration module
//!
//! Implements the `8004-reputation` extension for x402 protocol.
//! See: https://eips.ethereum.org/EIPS/eip-8004

pub mod types;
pub mod abi;
pub mod client;
pub mod extension;

pub use types::*;
pub use client::Erc8004Client;
pub use extension::parse_reputation_extension;
```

---

### Task 2: Define Rust Types

**File**: `src/erc8004/types.rs`

```rust
use serde::{Deserialize, Serialize};

/// CAIP-10 formatted agent registry reference
/// Format: "eip155:{chainId}:{contractAddress}"
pub type AgentRegistryRef = String;

/// Agent identity as declared in PaymentRequirements or PaymentPayload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdentity {
    /// CAIP-10 reference to the Identity Registry
    /// Example: "eip155:8453:0x7177a6867296406881E20d6647232314736Dd09A"
    pub agent_registry: AgentRegistryRef,

    /// Agent's NFT token ID in the Identity Registry
    pub agent_id: String,

    /// CAIP-10 reference to the Reputation Registry (optional)
    pub reputation_registry: Option<AgentRegistryRef>,

    /// Agent's service endpoint URL (optional)
    pub endpoint: Option<String>,
}

/// ERC-8004 extension data in PaymentRequirements
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReputationRequiredInfo {
    /// Server's agent identity
    #[serde(flatten)]
    pub agent_identity: AgentIdentity,
}

/// ERC-8004 extension data in PaymentPayload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReputationPayloadInfo {
    /// Server's identity (echoed from PaymentRequirements)
    pub server_identity: AgentIdentity,

    /// Client's identity (optional, enables bidirectional feedback)
    pub client_identity: Option<AgentIdentity>,
}

/// Proof of payment for feedback submission
/// Generated by facilitator after successful settlement
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofOfPayment {
    /// Transaction hash of the settlement
    pub tx_hash: String,

    /// Network in CAIP-2 format
    /// Example: "eip155:8453"
    pub network: String,

    /// Block number where tx was included
    pub block_number: u64,

    /// Block hash
    pub block_hash: String,

    /// Block timestamp (Unix seconds)
    pub timestamp: u64,

    /// Payer address (client)
    pub from: String,

    /// Recipient address (server)
    pub to: String,

    /// Amount transferred (in token's smallest unit)
    pub amount: String,

    /// Token contract address
    pub asset: String,
}

/// Feedback parameters for reputation submission
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackParams {
    /// Score from 0-100
    /// 100 = excellent, 50 = neutral, 0 = terrible
    pub score: u8,

    /// Primary tag (reason for feedback)
    /// Examples: "x402-resource-delivered", "x402-good-payer"
    pub tag1: String,

    /// Secondary tag (network identifier)
    /// Examples: "exact-evm", "exact-svm"
    pub tag2: String,

    /// Optional URI to detailed feedback file (IPFS or HTTPS)
    pub feedback_uri: Option<String>,

    /// Optional KECCAK-256 hash of feedback file
    pub feedback_hash: Option<String>,
}

/// Request body for POST /feedback endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRequest {
    /// Target agent's ID
    pub agent_id: String,

    /// CAIP-10 reference to Reputation Registry
    pub reputation_registry: AgentRegistryRef,

    /// Proof that payment actually occurred
    pub proof_of_payment: ProofOfPayment,

    /// Feedback details
    pub feedback: FeedbackParams,
}

/// Response from POST /feedback endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackResponse {
    /// Whether feedback was submitted successfully
    pub success: bool,

    /// Transaction hash of the giveFeedback call
    pub feedback_tx: Option<String>,

    /// Index of this feedback in the agent's feedback list
    pub feedback_index: Option<u64>,

    /// Error message if failed
    pub error: Option<String>,
}

/// Standardized feedback tags for x402 integration
pub mod tags {
    // Client → Server feedback tags (tag1)
    pub const RESOURCE_DELIVERED: &str = "x402-resource-delivered";
    pub const DELIVERY_FAILED: &str = "x402-delivery-failed";
    pub const DELIVERY_TIMEOUT: &str = "x402-delivery-timeout";
    pub const QUALITY_ISSUE: &str = "x402-quality-issue";

    // Server → Client feedback tags (tag1)
    pub const GOOD_PAYER: &str = "x402-good-payer";
    pub const PAYMENT_FAILED: &str = "x402-payment-failed";
    pub const INSUFFICIENT_FUNDS: &str = "x402-insufficient-funds";
    pub const INVALID_SIGNATURE: &str = "x402-invalid-signature";

    // Network identifiers (tag2)
    pub const EXACT_EVM: &str = "exact-evm";
    pub const EXACT_SVM: &str = "exact-svm";
}

/// ERC-8004 contract addresses (deterministic across all networks)
pub mod contracts {
    /// Identity Registry - ERC-721 agent registration
    pub const IDENTITY_REGISTRY: &str = "0x7177a6867296406881E20d6647232314736Dd09A";

    /// Reputation Registry - Feedback storage
    pub const REPUTATION_REGISTRY: &str = "0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322";

    /// Validation Registry - Independent verification
    pub const VALIDATION_REGISTRY: &str = "0x662b40A526cb4017d947e71eAF6753BF3eeE66d8";
}
```

---

### Task 3: Modify SettleResponse

**File**: `src/types.rs` (add to existing)

Add `ProofOfPayment` to the settle response:

```rust
// Add import at top
use crate::erc8004::ProofOfPayment;

// Modify SettleResponse struct
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResponse {
    pub success: bool,
    pub transaction: Option<String>,
    pub network: Option<String>,
    pub error: Option<String>,

    // NEW: ERC-8004 proof of payment for reputation feedback
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_of_payment: Option<ProofOfPayment>,
}
```

---

### Task 4: Capture ProofOfPayment in EVM Settlement

**File**: `src/chain/evm.rs`

After `transferWithAuthorization` succeeds, capture receipt details:

```rust
use crate::erc8004::ProofOfPayment;

// In settle_payment function, after tx confirmation:

async fn build_proof_of_payment(
    receipt: &TransactionReceipt,
    network: &Network,
    from: &Address,
    to: &Address,
    amount: &U256,
    asset: &Address,
) -> ProofOfPayment {
    let block = provider.get_block(receipt.block_number.unwrap()).await.unwrap();

    ProofOfPayment {
        tx_hash: format!("{:?}", receipt.transaction_hash),
        network: network.to_caip2(), // Need to implement this
        block_number: receipt.block_number.unwrap().as_u64(),
        block_hash: format!("{:?}", receipt.block_hash.unwrap()),
        timestamp: block.timestamp.as_u64(),
        from: format!("{:?}", from),
        to: format!("{:?}", to),
        amount: amount.to_string(),
        asset: format!("{:?}", asset),
    }
}
```

---

### Task 5: Create Reputation Registry ABI

**File**: `src/erc8004/abi.rs`

```rust
use alloy::sol;

// Reputation Registry ABI (subset needed for feedback)
sol! {
    #[sol(rpc)]
    interface IReputationRegistry {
        /// Submit feedback for an agent
        function giveFeedback(
            uint256 agentId,
            uint8 score,
            bytes32 tag1,
            bytes32 tag2,
            string calldata feedbackUri,
            bytes32 feedbackHash
        ) external;

        /// Get the last feedback index for a client
        function getLastIndex(
            uint256 agentId,
            address clientAddress
        ) external view returns (uint64);

        /// Get summary statistics for an agent
        function getSummary(
            uint256 agentId,
            address[] calldata clientAddresses,
            bytes32 tag1,
            bytes32 tag2
        ) external view returns (uint64 count, uint8 averageScore);

        /// Event emitted when feedback is submitted
        event NewFeedback(
            uint256 indexed agentId,
            address indexed clientAddress,
            uint8 score,
            bytes32 indexed tag1,
            bytes32 tag2,
            string feedbackUri,
            bytes32 feedbackHash
        );
    }
}

// Identity Registry ABI (for wallet verification)
sol! {
    #[sol(rpc)]
    interface IIdentityRegistry {
        /// Get the wallet address associated with an agent
        function getAgentWallet(uint256 agentId) external view returns (address);

        /// Get the owner of an agent NFT
        function ownerOf(uint256 tokenId) external view returns (address);

        /// Get agent metadata
        function getMetadata(
            uint256 agentId,
            string calldata key
        ) external view returns (bytes memory);
    }
}
```

---

### Task 6: Create ERC-8004 Client

**File**: `src/erc8004/client.rs`

```rust
use alloy::primitives::{Address, U256, FixedBytes};
use alloy::providers::Provider;
use alloy::transports::Transport;
use crate::erc8004::abi::{IReputationRegistry, IIdentityRegistry};
use crate::erc8004::types::*;

pub struct Erc8004Client<P, T> {
    provider: P,
    identity_registry: Address,
    reputation_registry: Address,
    signer: LocalWallet,
    _transport: std::marker::PhantomData<T>,
}

impl<P: Provider<T>, T: Transport + Clone> Erc8004Client<P, T> {
    pub fn new(
        provider: P,
        chain_id: u64,
        signer: LocalWallet,
    ) -> Self {
        Self {
            provider,
            identity_registry: contracts::IDENTITY_REGISTRY.parse().unwrap(),
            reputation_registry: contracts::REPUTATION_REGISTRY.parse().unwrap(),
            signer,
            _transport: std::marker::PhantomData,
        }
    }

    /// Submit feedback to the Reputation Registry
    pub async fn give_feedback(
        &self,
        agent_id: U256,
        feedback: &FeedbackParams,
    ) -> Result<(String, u64), Erc8004Error> {
        let contract = IReputationRegistry::new(
            self.reputation_registry,
            &self.provider,
        );

        // Convert tags to bytes32
        let tag1 = string_to_bytes32(&feedback.tag1);
        let tag2 = string_to_bytes32(&feedback.tag2);

        // Convert optional hash
        let feedback_hash = feedback.feedback_hash
            .as_ref()
            .map(|h| h.parse::<FixedBytes<32>>().unwrap_or_default())
            .unwrap_or_default();

        // Submit transaction
        let tx = contract.giveFeedback(
            agent_id,
            feedback.score,
            tag1,
            tag2,
            feedback.feedback_uri.clone().unwrap_or_default(),
            feedback_hash,
        );

        let pending = tx.send().await?;
        let receipt = pending.get_receipt().await?;

        // Get the feedback index from event logs
        let feedback_index = self.get_last_index(
            agent_id,
            self.signer.address(),
        ).await?;

        Ok((
            format!("{:?}", receipt.transaction_hash),
            feedback_index,
        ))
    }

    /// Get the wallet address registered for an agent
    pub async fn get_agent_wallet(
        &self,
        agent_id: U256,
    ) -> Result<Address, Erc8004Error> {
        let contract = IIdentityRegistry::new(
            self.identity_registry,
            &self.provider,
        );

        let wallet = contract.getAgentWallet(agent_id).call().await?;
        Ok(wallet._0)
    }

    /// Verify that payTo matches the agent's registered wallet
    pub async fn verify_agent_wallet(
        &self,
        agent_id: U256,
        expected_wallet: Address,
    ) -> Result<bool, Erc8004Error> {
        let registered_wallet = self.get_agent_wallet(agent_id).await?;
        Ok(registered_wallet == expected_wallet)
    }

    /// Get the last feedback index for a client
    async fn get_last_index(
        &self,
        agent_id: U256,
        client: Address,
    ) -> Result<u64, Erc8004Error> {
        let contract = IReputationRegistry::new(
            self.reputation_registry,
            &self.provider,
        );

        let index = contract.getLastIndex(agent_id, client).call().await?;
        Ok(index._0)
    }
}

/// Convert string to bytes32 (right-padded with zeros)
fn string_to_bytes32(s: &str) -> FixedBytes<32> {
    let mut bytes = [0u8; 32];
    let s_bytes = s.as_bytes();
    let len = std::cmp::min(s_bytes.len(), 32);
    bytes[..len].copy_from_slice(&s_bytes[..len]);
    FixedBytes::from(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum Erc8004Error {
    #[error("Contract call failed: {0}")]
    ContractError(String),

    #[error("Transaction failed: {0}")]
    TransactionError(String),

    #[error("Agent wallet mismatch: expected {expected}, got {actual}")]
    WalletMismatch {
        expected: String,
        actual: String,
    },
}
```

---

### Task 7: Add Feedback Handler

**File**: `src/handlers.rs` (add new handler)

```rust
use crate::erc8004::{FeedbackRequest, FeedbackResponse, Erc8004Client};

/// POST /feedback - Submit reputation feedback on-chain
pub async fn post_feedback(
    State(state): State<AppState>,
    Json(request): Json<FeedbackRequest>,
) -> Result<Json<FeedbackResponse>, StatusCode> {
    // Parse the reputation registry reference
    let (chain_id, registry_address) = parse_caip10(&request.reputation_registry)
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Get provider for the target network
    let network = Network::from_chain_id(chain_id)
        .ok_or(StatusCode::BAD_REQUEST)?;

    let provider = state.provider_cache
        .get_provider(&network)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify the proof of payment is valid
    // (check tx exists, matches claimed details)
    verify_proof_of_payment(&provider, &request.proof_of_payment)
        .await
        .map_err(|e| {
            tracing::error!("Invalid proof of payment: {}", e);
            StatusCode::BAD_REQUEST
        })?;

    // Create ERC-8004 client
    let signer = state.get_signer_for_network(&network)?;
    let client = Erc8004Client::new(provider, chain_id, signer);

    // Parse agent ID
    let agent_id: U256 = request.agent_id.parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Submit feedback
    match client.give_feedback(agent_id, &request.feedback).await {
        Ok((tx_hash, feedback_index)) => {
            tracing::info!(
                agent_id = %request.agent_id,
                tx_hash = %tx_hash,
                feedback_index = feedback_index,
                "Feedback submitted successfully"
            );

            Ok(Json(FeedbackResponse {
                success: true,
                feedback_tx: Some(tx_hash),
                feedback_index: Some(feedback_index),
                error: None,
            }))
        }
        Err(e) => {
            tracing::error!(
                agent_id = %request.agent_id,
                error = %e,
                "Failed to submit feedback"
            );

            Ok(Json(FeedbackResponse {
                success: false,
                feedback_tx: None,
                feedback_index: None,
                error: Some(e.to_string()),
            }))
        }
    }
}

/// Verify that a proof of payment is valid
async fn verify_proof_of_payment<P: Provider<T>, T: Transport>(
    provider: &P,
    proof: &ProofOfPayment,
) -> Result<(), String> {
    // Get the transaction receipt
    let tx_hash: B256 = proof.tx_hash.parse()
        .map_err(|_| "Invalid tx hash")?;

    let receipt = provider.get_transaction_receipt(tx_hash)
        .await
        .map_err(|e| format!("Failed to get receipt: {}", e))?
        .ok_or("Transaction not found")?;

    // Verify block number matches
    if receipt.block_number.unwrap_or_default().as_u64() != proof.block_number {
        return Err("Block number mismatch".to_string());
    }

    // Verify transaction was successful
    if receipt.status.unwrap_or_default() != 1 {
        return Err("Transaction failed".to_string());
    }

    Ok(())
}

/// Parse CAIP-10 reference: "eip155:{chainId}:{address}"
fn parse_caip10(caip10: &str) -> Result<(u64, Address), String> {
    let parts: Vec<&str> = caip10.split(':').collect();
    if parts.len() != 3 || parts[0] != "eip155" {
        return Err("Invalid CAIP-10 format".to_string());
    }

    let chain_id: u64 = parts[1].parse()
        .map_err(|_| "Invalid chain ID")?;

    let address: Address = parts[2].parse()
        .map_err(|_| "Invalid address")?;

    Ok((chain_id, address))
}
```

---

### Task 8: Register Route

**File**: `src/main.rs` (add to router)

```rust
use crate::handlers::post_feedback;

// In router setup:
let app = Router::new()
    .route("/", get(get_index))
    .route("/health", get(health))
    .route("/supported", get(get_supported))
    .route("/verify", post(post_verify))
    .route("/settle", post(post_settle))
    // NEW: ERC-8004 feedback endpoint
    .route("/feedback", post(post_feedback))
    .with_state(state);
```

---

### Task 9: Add Network CAIP-2 Helper

**File**: `src/network.rs` (add method)

```rust
impl Network {
    /// Convert to CAIP-2 format: "eip155:{chainId}"
    pub fn to_caip2(&self) -> String {
        match self.family() {
            NetworkFamily::Evm => format!("eip155:{}", self.chain_id()),
            NetworkFamily::Solana => {
                // Solana uses different CAIP-2 format
                match self {
                    Network::SolanaMainnet => "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
                    Network::SolanaDevnet => "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
                    _ => unreachable!(),
                }
            }
        }
    }

    /// Parse from CAIP-2 format
    pub fn from_caip2(caip2: &str) -> Option<Self> {
        let parts: Vec<&str> = caip2.split(':').collect();
        if parts.len() < 2 {
            return None;
        }

        match parts[0] {
            "eip155" => {
                let chain_id: u64 = parts[1].parse().ok()?;
                Self::from_chain_id(chain_id)
            }
            "solana" => {
                match parts[1] {
                    "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" => Some(Network::SolanaMainnet),
                    "EtWTRABZaYq6iMfeYKouRu166VU2xqa1" => Some(Network::SolanaDevnet),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
```

---

## 7. Solidity ABIs

### Identity Registry ABI (Minimal)

```json
[
  {
    "inputs": [{"name": "agentId", "type": "uint256"}],
    "name": "getAgentWallet",
    "outputs": [{"name": "", "type": "address"}],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "inputs": [{"name": "tokenId", "type": "uint256"}],
    "name": "ownerOf",
    "outputs": [{"name": "", "type": "address"}],
    "stateMutability": "view",
    "type": "function"
  }
]
```

### Reputation Registry ABI (Minimal)

```json
[
  {
    "inputs": [
      {"name": "agentId", "type": "uint256"},
      {"name": "score", "type": "uint8"},
      {"name": "tag1", "type": "bytes32"},
      {"name": "tag2", "type": "bytes32"},
      {"name": "feedbackUri", "type": "string"},
      {"name": "feedbackHash", "type": "bytes32"}
    ],
    "name": "giveFeedback",
    "outputs": [],
    "stateMutability": "nonpayable",
    "type": "function"
  },
  {
    "inputs": [
      {"name": "agentId", "type": "uint256"},
      {"name": "clientAddress", "type": "address"}
    ],
    "name": "getLastIndex",
    "outputs": [{"name": "", "type": "uint64"}],
    "stateMutability": "view",
    "type": "function"
  },
  {
    "anonymous": false,
    "inputs": [
      {"indexed": true, "name": "agentId", "type": "uint256"},
      {"indexed": true, "name": "clientAddress", "type": "address"},
      {"indexed": false, "name": "score", "type": "uint8"},
      {"indexed": true, "name": "tag1", "type": "bytes32"},
      {"indexed": false, "name": "tag2", "type": "bytes32"},
      {"indexed": false, "name": "feedbackUri", "type": "string"},
      {"indexed": false, "name": "feedbackHash", "type": "bytes32"}
    ],
    "name": "NewFeedback",
    "type": "event"
  }
]
```

---

## 8. Rust Type Definitions

See [Task 2](#task-2-define-rust-types) for complete type definitions.

---

## 9. Testing Plan

### 9.1 Unit Tests

| Test | Description |
|------|-------------|
| `test_parse_caip10` | Parse "eip155:8453:0x..." correctly |
| `test_parse_caip2` | Parse "eip155:8453" correctly |
| `test_string_to_bytes32` | Convert tag strings to bytes32 |
| `test_proof_of_payment_serialization` | JSON round-trip |

### 9.2 Integration Tests (Base Sepolia)

```bash
# Test settlement returns ProofOfPayment
python tests/integration/test_erc8004_proof.py --network base-sepolia

# Test feedback submission
python tests/integration/test_erc8004_feedback.py --network base-sepolia
```

**Test Script**: `tests/integration/test_erc8004_feedback.py`

```python
#!/usr/bin/env python3
"""
Test ERC-8004 feedback submission on Base Sepolia.
Requires:
- Running facilitator
- ERC-8004 contracts deployed on Base Sepolia
- Test agent registered in Identity Registry
"""

import requests
import json
from web3 import Web3

FACILITATOR_URL = "http://localhost:8080"
BASE_SEPOLIA_RPC = "https://sepolia.base.org"

# ERC-8004 contracts (same on all networks)
IDENTITY_REGISTRY = "0x7177a6867296406881E20d6647232314736Dd09A"
REPUTATION_REGISTRY = "0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322"

# Test agent (must be registered first)
TEST_AGENT_ID = "1"

def test_feedback_submission():
    """Test submitting feedback after a settlement."""

    # First, do a settlement to get ProofOfPayment
    settle_response = do_test_settlement()
    assert settle_response["success"], "Settlement failed"

    proof = settle_response["proofOfPayment"]
    assert proof is not None, "No ProofOfPayment in response"

    # Now submit feedback
    feedback_request = {
        "agentId": TEST_AGENT_ID,
        "reputationRegistry": f"eip155:84532:{REPUTATION_REGISTRY}",
        "proofOfPayment": proof,
        "feedback": {
            "score": 100,
            "tag1": "x402-resource-delivered",
            "tag2": "exact-evm",
            "feedbackUri": None,
            "feedbackHash": None
        }
    }

    response = requests.post(
        f"{FACILITATOR_URL}/feedback",
        json=feedback_request
    )

    result = response.json()
    print(f"Feedback response: {json.dumps(result, indent=2)}")

    assert result["success"], f"Feedback failed: {result.get('error')}"
    assert result["feedbackTx"] is not None, "No feedback tx hash"
    assert result["feedbackIndex"] is not None, "No feedback index"

    # Verify on-chain
    verify_feedback_on_chain(
        TEST_AGENT_ID,
        result["feedbackIndex"]
    )

    print("SUCCESS: Feedback submitted and verified on-chain")

def do_test_settlement():
    """Execute a test settlement and return response."""
    # Implementation depends on existing test setup
    pass

def verify_feedback_on_chain(agent_id, feedback_index):
    """Verify feedback exists in Reputation Registry."""
    w3 = Web3(Web3.HTTPProvider(BASE_SEPOLIA_RPC))
    # Call readFeedback and verify
    pass

if __name__ == "__main__":
    test_feedback_submission()
```

### 9.3 Mainnet Verification (Post-Launch)

```bash
# Verify contracts deployed at expected addresses
cast call 0x7177a6867296406881E20d6647232314736Dd09A \
  "name()" --rpc-url https://mainnet.base.org

# Should return the contract name
```

---

## 10. Deployment Checklist

### Pre-Deployment (Before Thursday)

- [ ] All code changes complete and tested on testnet
- [ ] Docker image built with ERC-8004 support
- [ ] Integration tests passing on Base Sepolia
- [ ] Documentation updated

### Deployment Day (Thursday 9:00 AM)

1. **Verify mainnet contracts** (9:00 AM)
   ```bash
   # Check Identity Registry is live
   cast call 0x7177a6867296406881E20d6647232314736Dd09A \
     "name()" --rpc-url https://mainnet.base.org
   ```

2. **Deploy facilitator** (9:15 AM)
   ```bash
   ./scripts/build-and-push.sh v1.X.0
   aws ecs update-service --cluster facilitator-production \
     --service facilitator-production --force-new-deployment
   ```

3. **Smoke test** (9:30 AM)
   ```bash
   # Test settlement includes ProofOfPayment
   curl -X POST https://facilitator.ultravioletadao.xyz/settle \
     -H "Content-Type: application/json" \
     -d '{"x402Version":1,...}'
   ```

4. **Monitor** (ongoing)
   - Watch CloudWatch for errors
   - Check first feedback submissions
   - Monitor gas costs

---

## 11. References

### Official Documentation
- [ERC-8004: Trustless Agents (EIP)](https://eips.ethereum.org/EIPS/eip-8004)
- [ERC-8004 Discussion (Ethereum Magicians)](https://ethereum-magicians.org/t/erc-8004-trustless-agents/25098)

### x402 Integration Proposal
- [GitHub Issue #931: 8004-reputation extension](https://github.com/coinbase/x402/issues/931)
- [GitHub PR #1024: Reputation extension spec](https://github.com/coinbase/x402/pull/1024)

### Reference Implementations
- [ChaosChain/trustless-agents-erc-ri](https://github.com/ChaosChain/trustless-agents-erc-ri) - Official reference
- [nuwa-protocol/nuwa-8004](https://github.com/nuwa-protocol/nuwa-8004) - Alternative implementation
- [awesome-erc8004](https://github.com/sudeepb02/awesome-erc8004) - Curated resources

### Contract Explorers (Testnet)
- [Base Sepolia Identity Registry](https://sepolia.basescan.org/address/0x7177a6867296406881E20d6647232314736Dd09A)
- [Base Sepolia Reputation Registry](https://sepolia.basescan.org/address/0xB5048e3ef1DA4E04deB6f7d0423D06F63869e322)

---

## Appendix A: Timeline

| Day | Date | Tasks | Deliverable |
|-----|------|-------|-------------|
| **Day 1** | Tue Jan 27 | Phase 1 + Phase 2 | ProofOfPayment in /settle, extension parsing |
| **Day 2** | Wed Jan 28 | Phase 3 | /feedback endpoint working on testnet |
| **Day 3** | Thu Jan 29 AM | Phase 5 | Production deployment, mainnet verification |

---

## Appendix B: Gas Cost Estimates

| Operation | Estimated Gas | @ 0.1 gwei | @ 1 gwei |
|-----------|---------------|------------|----------|
| giveFeedback | ~276,000 | ~$0.01 | ~$0.10 |
| getAgentWallet (view) | 0 | $0 | $0 |
| getSummary (view) | 0 | $0 | $0 |

**Note**: Feedback submission requires the client/server to pay gas, not the facilitator. The facilitator only acts as a relay.

---

*Document created: January 26, 2025*
*Last updated: January 26, 2025*
*Author: Claude (Ultravioleta DAO)*
