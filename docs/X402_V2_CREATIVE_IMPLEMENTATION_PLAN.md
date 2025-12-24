# x402 v2 Creative Implementation Plan

**Created**: 2025-12-23
**Updated**: 2025-12-23 (Re-analysis with all alternatives)
**Status**: Research Complete - All Alternatives Evaluated
**Author**: Deep research combining x402 spec, CAIP-122, EIP-4361, Cloudflare proposals, and Superfluid patterns

---

## Executive Summary

This document presents a **creative, performance-first** implementation plan for x402 v2 advanced features. Key innovations:

1. **Zero-infrastructure sessions** using Moka in-memory cache (no Redis required for single-instance)
2. **Hybrid session architecture** that scales from embedded to distributed seamlessly
3. **Pragmatic dynamic pricing** using EIP-3009's random nonce design
4. **Deferred tabs** inspired by Cloudflare's approach (no database required initially)
5. **Future-ready streaming** architecture compatible with Superfluid integration

---

## Table of Contents

1. [Current State Analysis](#1-current-state-analysis)
2. [Payment Schemes Deep Dive](#2-payment-schemes-deep-dive)
3. [SIWx Sessions Architecture](#3-siwx-sessions-architecture)
4. [Lightweight Infrastructure Strategy](#4-lightweight-infrastructure-strategy)
5. [Alternative Approaches Analysis](#5-alternative-approaches-analysis) *(NEW)*
6. [Implementation Phases (Revised)](#6-implementation-phases-revised)
7. [Creative Solutions](#7-creative-solutions)
8. [Performance Analysis](#8-performance-analysis)
9. [Security Considerations](#9-security-considerations)
10. [Code Architecture](#10-code-architecture)
11. [Decision Matrix](#11-decision-matrix)

---

## 1. Current State Analysis

### 1.1 Existing Schemes

| Scheme | Status | Implementation |
|--------|--------|----------------|
| `exact` | Production | EIP-3009 `transferWithAuthorization` |
| `fhe-transfer` | Production | Proxied to Zama Lambda |

### 1.2 Current Architecture Strengths

From analyzing `src/types.rs`, `src/facilitator.rs`, and `src/handlers.rs`:

```
Request Flow:
  Client -> POST /settle -> handlers.rs -> Facilitator trait -> chain/evm.rs -> RPC
                                                             -> chain/solana.rs -> RPC
```

**Key insights:**
- **Stateless design**: No session state maintained between requests
- **Trait-based extensibility**: `Facilitator` trait allows scheme-specific implementations
- **Multi-chain support**: `ExactPaymentPayload` enum handles EVM, Solana, NEAR, Stellar
- **Random nonces**: EIP-3009 uses `bytes32` random nonces (not sequential) - **this is crucial for concurrent authorizations**

### 1.3 Extension Points for v2

```rust
// src/types.rs:104-110 - Scheme enum is the primary extension point
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Exact,
    #[serde(rename = "fhe-transfer")]
    FheTransfer,
    // NEW SCHEMES GO HERE
}
```

---

## 2. Payment Schemes Deep Dive

### 2.1 Scheme Compatibility Matrix

| Scheme | EIP-3009 Compatible | Requires State | Requires Contracts | Feasibility |
|--------|---------------------|----------------|-------------------|-------------|
| `exact` | Native | No | No | Production |
| `deferred` | Yes (delayed settle) | Yes (tabs) | No | HIGH |
| `upto` | Partial* | Yes | Optional | MEDIUM |
| `subscription` | Yes (pre-signed) | Yes | No | MEDIUM |
| `streaming` | No | Yes | Yes (Superfluid) | LOW |

*`upto` requires creative workaround - see section 6.1

### 2.2 New Scheme: `deferred` (Cloudflare Model)

**Concept**: Accumulate charges, settle later in batch.

```rust
/// Deferred payment scheme - accumulate charges on a "tab"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredSchemePayload {
    /// Tab identifier (created on first request)
    pub tab_id: Option<String>,
    /// HTTP Message Signature for identity verification
    pub signature: String,
    /// Signature input header value
    pub signature_input: String,
    /// Public key ID for verification
    pub key_id: String,
}
```

**Protocol Flow** (based on Cloudflare's proposal):

```
1. Client -> Server: Request resource (no payment yet)
2. Server -> Client: 402 with deferred scheme, includes tab creation info
3. Client -> Server: Signed request with HTTP Message Signature
4. Server: Validates signature, creates/updates tab, returns resource
5. [Repeat 3-4 for multiple requests]
6. Server -> Client: Tab settlement request (when threshold reached)
7. Client -> Server: EIP-3009 authorization for total amount
8. Server: Settles on-chain, closes tab
```

### 2.3 New Scheme: `upto` (Dynamic Pricing)

**The EIP-3009 Challenge**:
```rust
// EIP-3009 signature covers EXACT value - cannot be modified
struct TransferWithAuthorization {
    from: address,
    to: address,
    value: uint256,      // <-- This is signed! Cannot change after
    validAfter: uint256,
    validBefore: uint256,
    nonce: bytes32,
}
```

**Creative Solution**: Pre-authorize maximum, track actual usage, batch refund difference.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoSchemePayload {
    /// Full EIP-3009 authorization for MAX amount
    pub max_authorization: ExactEvmPayload,
    /// Estimated usage (for UX only, not enforced)
    pub estimated_cost: Option<TokenAmount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoSettleResponse {
    /// Standard settle response
    #[serde(flatten)]
    pub base: SettleResponse,
    /// Actual amount charged (may be < max)
    pub actual_charged: TokenAmount,
    /// Refund amount (max - actual)
    pub refund_amount: TokenAmount,
    /// Refund status
    pub refund_status: RefundStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RefundStatus {
    /// Refund will be processed in next batch (monthly)
    Pending,
    /// Refund processed (includes tx hash)
    Completed { transaction: TransactionHash },
    /// No refund needed (actual == max)
    NotApplicable,
}
```

### 2.4 New Scheme: `subscription`

**Concept**: Client pre-signs multiple period authorizations upfront.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSchemePayload {
    /// Subscription identifier
    pub subscription_id: String,
    /// Current period being claimed (0-indexed)
    pub period: u32,
    /// Pre-signed authorization for this specific period
    pub period_authorization: ExactEvmPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSetupRequest {
    /// Subscriber wallet address
    pub subscriber: MixedAddress,
    /// Amount per period in base units
    pub amount_per_period: TokenAmount,
    /// Period duration in seconds (e.g., 2592000 = 30 days)
    pub period_seconds: u64,
    /// Total number of periods (e.g., 12 for annual)
    pub total_periods: u32,
    /// Pre-signed authorizations for ALL periods
    /// Each has unique nonce (EIP-3009 random nonces enable this!)
    pub period_authorizations: Vec<ExactEvmPayload>,
}
```

**Why this works with EIP-3009**:
- Random `bytes32` nonces allow generating unlimited concurrent authorizations
- Each period has its own independent authorization
- No sequential nonce bottleneck

---

## 3. SIWx Sessions Architecture

### 3.1 Why Server-Issued JWTs Beat Spec's SIGNED-IDENTIFIER

**x402 v2 Spec Proposal** (problematic):
```
Every request requires: SIGNED-IDENTIFIER header with wallet signature
Problems:
- Smart accounts (EIP-1271) require RPC call per request (~200ms)
- Forces client to sign every request
- No caching benefit
```

**Our Approach** (pragmatic):
```
First request: Settle payment, receive JWT session token
Subsequent requests: Include JWT in Authorization header
Benefits:
- HMAC validation: ~5 microseconds (not 200ms)
- Works with smart accounts (signature verified once at settlement)
- Standard OAuth2-style flow
```

### 3.2 Session Claims Structure

```rust
use serde::{Deserialize, Serialize};

/// JWT claims for x402 session tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X402SessionClaims {
    // Standard JWT claims
    /// Subject: wallet address (CAIP-10 format)
    pub sub: String,
    /// Issued at: Unix timestamp
    pub iat: u64,
    /// Expiration: Unix timestamp
    pub exp: u64,
    /// Issuer: facilitator URL
    pub iss: String,

    // x402-specific claims
    /// Network where payment was settled (CAIP-2 format)
    pub network: String,
    /// Settlement transaction hash (proof of payment)
    pub payment_tx: String,
    /// Amount paid (for access level determination)
    pub amount: String,
    /// Resource URI this session grants access to
    pub resource: String,
    /// Optional: Scopes/permissions granted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
}
```

### 3.3 Multi-Chain Identity (CAIP-122 Alignment)

```rust
/// Supported signature types for SIWx
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SiwxSignature {
    /// Ethereum EOA (personal_sign / EIP-191)
    #[serde(rename = "eip191")]
    Eip191 { signature: String },
    /// Ethereum smart account (EIP-1271)
    #[serde(rename = "eip1271")]
    Eip1271 { signature: String, account: String },
    /// Solana (ed25519)
    #[serde(rename = "solana")]
    Solana { signature: String },
    /// NEAR (ed25519)
    #[serde(rename = "near")]
    Near { signature: String },
}

/// SIWx message following CAIP-122 structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiwxMessage {
    /// Domain requesting authentication
    pub domain: String,
    /// Account address (chain-specific format)
    pub address: String,
    /// Human-readable statement
    pub statement: Option<String>,
    /// Resource URI
    pub uri: String,
    /// Protocol version
    pub version: String,
    /// Chain ID (CAIP-2 format: "eip155:8453")
    pub chain_id: String,
    /// Random nonce for replay protection
    pub nonce: String,
    /// Issuance timestamp (RFC 3339)
    pub issued_at: String,
    /// Expiration timestamp (RFC 3339)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_time: Option<String>,
}
```

---

## 4. Lightweight Infrastructure Strategy

### 4.1 Zero-Infrastructure Session Store (Phase 1)

**Use Moka in-memory cache instead of Redis**:

```rust
use moka::future::Cache;
use std::time::Duration;

/// In-memory session store using Moka
/// Suitable for single-instance deployments
pub struct MokaSessionStore {
    /// Session cache with TTL
    sessions: Cache<String, X402SessionClaims>,
    /// Revocation set (token hashes that have been invalidated)
    revoked: Cache<String, ()>,
}

impl MokaSessionStore {
    pub fn new(max_sessions: u64, default_ttl: Duration) -> Self {
        Self {
            sessions: Cache::builder()
                .max_capacity(max_sessions)
                .time_to_live(default_ttl)
                .build(),
            revoked: Cache::builder()
                .max_capacity(max_sessions / 10) // 10% for revocations
                .time_to_live(default_ttl * 2)   // Keep revocations longer
                .build(),
        }
    }

    pub async fn store(&self, token_hash: String, claims: X402SessionClaims) {
        self.sessions.insert(token_hash, claims).await;
    }

    pub async fn validate(&self, token_hash: &str) -> Option<X402SessionClaims> {
        // Check revocation first
        if self.revoked.get(token_hash).await.is_some() {
            return None;
        }
        self.sessions.get(token_hash).await
    }

    pub async fn revoke(&self, token_hash: &str) {
        self.sessions.invalidate(token_hash).await;
        self.revoked.insert(token_hash.to_string(), ()).await;
    }
}
```

**Benefits**:
- **Zero infrastructure**: No Redis deployment needed
- **~5 microsecond lookups**: In-process memory access
- **Automatic TTL cleanup**: Moka handles expiration
- **~10MB per 10k sessions**: Efficient memory usage

**Limitations**:
- Sessions lost on restart (acceptable for short-lived sessions)
- Single instance only (fine for current ECS deployment)

### 4.2 Hybrid Store Pattern (Phase 2 - When Needed)

```rust
use async_trait::async_trait;

/// Abstract session store trait for pluggable backends
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn store(&self, token_hash: String, claims: X402SessionClaims) -> Result<(), StoreError>;
    async fn get(&self, token_hash: &str) -> Result<Option<X402SessionClaims>, StoreError>;
    async fn revoke(&self, token_hash: &str) -> Result<(), StoreError>;
}

/// Tiered cache: L1 (Moka) + L2 (Redis)
/// Only add Redis when horizontal scaling is needed
pub struct TieredSessionStore {
    l1: MokaSessionStore,                    // Fast, local
    l2: Option<Arc<dyn SessionStore>>,       // Optional distributed
}

impl TieredSessionStore {
    pub fn embedded_only(max_sessions: u64, ttl: Duration) -> Self {
        Self {
            l1: MokaSessionStore::new(max_sessions, ttl),
            l2: None,
        }
    }

    pub fn with_redis(l1: MokaSessionStore, redis: impl SessionStore + 'static) -> Self {
        Self {
            l1,
            l2: Some(Arc::new(redis)),
        }
    }
}
```

### 4.3 In-Memory Deferred Tabs (Phase 1)

```rust
use std::collections::HashMap;
use tokio::sync::RwLock;
use std::time::Instant;

/// Lightweight in-memory tab storage
/// Suitable for low-volume deployments
pub struct InMemoryTabStore {
    tabs: RwLock<HashMap<String, DeferredTab>>,
    /// Auto-cleanup threshold
    max_tabs: usize,
    /// Max tab age before forced settlement
    max_age: Duration,
}

#[derive(Debug, Clone)]
pub struct DeferredTab {
    pub id: String,
    pub payer: MixedAddress,
    pub network: String,
    pub total_owed: TokenAmount,
    pub charges: Vec<TabCharge>,
    pub created_at: Instant,
    pub last_activity: Instant,
}

#[derive(Debug, Clone)]
pub struct TabCharge {
    pub timestamp: Instant,
    pub amount: TokenAmount,
    pub description: String,
    pub request_id: Option<String>,
}

impl InMemoryTabStore {
    pub async fn create_tab(&self, payer: MixedAddress, network: String) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let tab = DeferredTab {
            id: id.clone(),
            payer,
            network,
            total_owed: TokenAmount::from(0u64),
            charges: Vec::new(),
            created_at: Instant::now(),
            last_activity: Instant::now(),
        };

        let mut tabs = self.tabs.write().await;

        // Cleanup old tabs if at capacity
        if tabs.len() >= self.max_tabs {
            self.cleanup_stale_tabs(&mut tabs);
        }

        tabs.insert(id.clone(), tab);
        id
    }

    pub async fn add_charge(
        &self,
        tab_id: &str,
        amount: TokenAmount,
        description: String,
    ) -> Result<TokenAmount, TabError> {
        let mut tabs = self.tabs.write().await;
        let tab = tabs.get_mut(tab_id).ok_or(TabError::NotFound)?;

        tab.charges.push(TabCharge {
            timestamp: Instant::now(),
            amount,
            description,
            request_id: None,
        });
        tab.total_owed = tab.total_owed + amount;
        tab.last_activity = Instant::now();

        Ok(tab.total_owed)
    }

    fn cleanup_stale_tabs(&self, tabs: &mut HashMap<String, DeferredTab>) {
        let now = Instant::now();
        tabs.retain(|_, tab| now.duration_since(tab.last_activity) < self.max_age);
    }
}
```

---

## 5. Alternative Approaches Analysis

This section evaluates ALL discovered alternatives before recommending the implementation approach.

### 5.1 Session Storage Alternatives

| Option | Latency | Complexity | Memory | Persistence | Best For |
|--------|---------|------------|--------|-------------|----------|
| **Moka** | ~5μs | Low | 10KB/session | No | High-performance, single instance |
| **DashMap** | ~2μs | Very Low | 10KB/session | No | Simple cases, <10K sessions |
| **tower-sessions** | ~10μs | Medium | Varies | Optional | Axum-native, pluggable backends |
| **AxumSession** | ~10μs | Medium | Varies | Optional | More features than tower-sessions |
| **Redis** | 1-5ms | Medium | External | Yes | Multi-instance, persistence |

**Detailed Comparison**:

```rust
// Option A: Moka (RECOMMENDED for Phase 1)
// Pro: Highest performance, zero infrastructure
// Con: Sessions lost on restart
use moka::future::Cache;
let sessions: Cache<String, Claims> = Cache::builder()
    .max_capacity(100_000)
    .time_to_live(Duration::from_secs(3600))
    .build();

// Option B: DashMap (Simpler alternative)
// Pro: Even simpler, std-lib-like API
// Con: No automatic TTL eviction - must implement manually
use dashmap::DashMap;
let sessions: DashMap<String, (Claims, Instant)> = DashMap::new();

// Option C: tower-sessions (Axum-native)
// Pro: Middleware-based, clean separation
// Con: Slightly more complex setup
use tower_sessions::{MemoryStore, SessionManagerLayer};
let session_store = MemoryStore::default();
let session_layer = SessionManagerLayer::new(session_store);

// Option D: Redis (Future scaling)
// Pro: Persistence, multi-instance, atomic ops
// Con: Network latency, additional infrastructure
use redis::AsyncCommands;
conn.set_ex("session:xyz", claims_json, 3600).await?;
```

**Decision**: Start with **Moka** for zero-infrastructure deployment. Add DashMap-to-Moka migration path if needed for debugging (DashMap is easier to inspect). Add Redis layer when horizontal scaling is required.

### 5.2 Dynamic Pricing Alternatives

| Option | EIP-3009 Compatible | Trust Required | Gas Efficiency | Complexity |
|--------|---------------------|----------------|----------------|------------|
| **Authorize Max + Refund** | Yes | Server honesty | High (batch refunds) | Medium |
| **Prepaid Credits** | Yes | Low | High | Low |
| **ERC-7521 Intents** | No (new standard) | Low | Medium | High |
| **Permit2 Batch** | Partial | Low | High | High |
| **Payment Channels** | No | Low | Very High | Very High |

**Detailed Comparison**:

```rust
// Option A: Authorize Max, Settle Max, Refund Difference (RECOMMENDED)
// Pro: Works with existing EIP-3009, no contract changes
// Con: Users must trust facilitator for refunds
struct UptoSettlement {
    max_authorized: U256,    // What client signed
    actual_cost: U256,       // What was really used
    refund_due: U256,        // max - actual
    refund_status: RefundStatus,
}
// Batch refunds monthly to save gas

// Option B: Prepaid Credits (SIMPLER MVP)
// Pro: No refunds needed, very simple
// Con: Requires upfront capital from users
struct PrepaidAccount {
    wallet: Address,
    balance: U256,           // Credits purchased
    spent: U256,             // Credits used
}
// User buys $10 credits, each API call deducts from balance
// No refunds - credits are spent or expire

// Option C: ERC-7521 Intents (FUTURE)
// Pro: Built-in variable amounts, chain-level guarantees
// Con: Requires wallet support, not widely adopted
// See: https://eips.ethereum.org/EIPS/eip-7521
struct Intent {
    to: Address,
    min_value: U256,         // Minimum payment
    max_value: U256,         // Maximum payment
    // Solver determines actual within bounds
}

// Option D: Permit2 Batch (ADVANCED)
// Pro: One signature for multiple future payments
// Con: Complex integration, Uniswap ecosystem
// See: https://docs.uniswap.org/contracts/permit2/overview
```

**Decision**: **Authorize Max + Refund** for full dynamic pricing support. Consider **Prepaid Credits** as simpler MVP option if trust is a concern.

### 5.3 Deferred Payments Alternatives

| Option | State Required | Complexity | Settlement | Best For |
|--------|----------------|------------|------------|----------|
| **In-Memory Tabs** | Yes (memory) | Low | On-demand | Low volume |
| **HTTP Signatures (RFC 9421)** | Yes | Medium | Batch | API billing |
| **Payment Channels** | Yes (on-chain) | Very High | Instant | High frequency |
| **State Channels** | Yes (on-chain) | Very High | Batch | Gaming, NFTs |

**Detailed Comparison**:

```rust
// Option A: In-Memory Tabs (RECOMMENDED for MVP)
// Pro: Simple, zero infrastructure
// Con: Tabs lost on restart (acceptable for short-lived tabs)
struct Tab {
    payer: Address,
    charges: Vec<Charge>,
    total: U256,
}
// Settle when: threshold reached OR daily/weekly cron

// Option B: HTTP Message Signatures (RFC 9421)
// Pro: Cloudflare-compatible, standard-based
// Con: More complex signature verification
// Used for identity verification without on-chain check
let signature = HttpSignature::verify(request)?;
let payer = signature.key_id.parse::<Address>()?;

// Option C: Payment Channels (NOT RECOMMENDED)
// Pro: Instant finality, minimal gas
// Con: Requires smart contract, channel management
// Overkill for current use case

// Option D: State Channels (NOT RECOMMENDED)
// Pro: Arbitrary state transitions, gaming use cases
// Con: Very complex, requires dispute resolution
// Not suitable for simple payment aggregation
```

**Decision**: **In-Memory Tabs** for MVP. Add HTTP Message Signatures later for Cloudflare compatibility.

### 5.4 Simpler MVP Alternatives

If full v2 features are too complex, consider these graduated approaches:

**MVP Level 1: Payment Receipts Only (Simplest)**
```rust
// No sessions, no state - just return proof of payment
struct SettleResponseWithReceipt {
    #[serde(flatten)]
    base: SettleResponse,
    /// JWT containing payment proof - client stores it
    receipt: String,
}
// Client presents receipt on subsequent requests
// Server validates JWT signature (stateless!)
// No session store needed
```
**Effort**: ~2 days. **Benefit**: Clients can prove payment without sessions.

**MVP Level 2: Sessions Without SIWx**
```rust
// Skip full CAIP-122 SIWx - use simple JWT after settle
// No additional wallet signatures needed
struct SimpleSession {
    sub: String,          // wallet address
    payment_tx: String,   // proof of payment
    exp: u64,             // expiration
}
// Issue on settle, validate on subsequent requests
// Much simpler than full SIWx with nonces, domains, etc.
```
**Effort**: ~1 week. **Benefit**: Pay-once-access pattern without SIWx complexity.

**MVP Level 3: Full SIWx (Original Plan)**
- Full CAIP-122 compliance
- Nonce management
- Domain binding
- Multi-chain identity verification

**Effort**: ~3-4 weeks. **Benefit**: Full spec compliance, cross-platform identity.

### 5.5 Recommendation Summary

| Feature | MVP (Fastest) | Balanced | Full Spec |
|---------|---------------|----------|-----------|
| Sessions | Payment Receipt JWT | Simple JWT Sessions | Full SIWx |
| Storage | None (stateless) | Moka | Moka + Redis |
| Dynamic Pricing | Skip (exact only) | Prepaid Credits | Authorize Max + Refund |
| Deferred | Skip | In-Memory Tabs | HTTP Signatures + DB |
| Subscriptions | Skip | Pre-signed EIP-3009 | Full management APIs |
| **Effort** | 1 week | 4-6 weeks | 12+ weeks |
| **Infra Cost** | $0 | $0 | $0-25/mo |

**Recommended Path**: Start with **Balanced** approach - Simple JWT Sessions + Moka + In-Memory Tabs. This provides 80% of the value with 30% of the complexity.

---

## 6. Implementation Phases (Revised)

### Phase 1: Sessions + Deferred (Weeks 1-8)

**Goal**: Enable pay-once-reuse-access and tab-based billing with zero new infrastructure.

```
Week 1-2: Session Types & JWT Integration
- Add X402SessionClaims to src/types.rs
- Add SessionManager with HMAC signing/verification
- Add MokaSessionStore

Week 3-4: Handler Integration
- Modify POST /settle to return session token on success
- Add session validation middleware
- Add GET /session/validate endpoint

Week 5-6: Deferred Scheme
- Add DeferredSchemePayload to Scheme enum
- Add InMemoryTabStore
- Add POST /tabs, POST /tabs/{id}/charge, POST /tabs/{id}/settle endpoints

Week 7-8: Testing & Documentation
- Integration tests for session flow
- Integration tests for deferred tabs
- Update API documentation
```

**New Dependencies**:
```toml
[dependencies]
jsonwebtoken = "9.2"      # JWT encoding/validation
moka = { version = "0.12", features = ["future"] }  # In-memory cache
uuid = { version = "1.6", features = ["v4"] }       # Tab IDs
```

**Infrastructure Cost**: $0 (all in-memory)

### Phase 2: Upto Scheme + Refunds (Weeks 9-16)

**Goal**: Enable dynamic pricing for AI/compute use cases.

```
Week 9-10: Upto Scheme Types
- Add UptoSchemePayload
- Add RefundEntry tracking
- Add in-memory refund ledger

Week 11-12: Settlement Flow
- Settle max amount on-chain
- Track actual usage
- Record refund entries

Week 13-14: Batch Refund Processor
- Background task to batch refunds
- Configurable thresholds (amount or time-based)
- Refund transaction submission

Week 15-16: Monitoring & Alerts
- Prometheus metrics for refund backlog
- Alerts for failed refunds
- Dashboard for refund status
```

**Infrastructure Cost**: $0 (in-memory refund ledger with file persistence)

### Phase 3: Subscriptions (Weeks 17-24)

**Goal**: Enable recurring payments with pre-signed authorizations.

```
Week 17-18: Subscription Types
- Add SubscriptionSchemePayload
- Add SubscriptionSetupRequest
- Add in-memory subscription registry

Week 19-20: Setup Flow
- Endpoint to create subscription
- Validate all period authorizations upfront
- Store subscription metadata

Week 21-22: Settlement Cron
- Background task to check due subscriptions
- Settle current period authorization
- Handle failures with retry logic

Week 23-24: Management APIs
- GET /subscriptions/{id} status
- DELETE /subscriptions/{id} cancellation
- Webhook notifications
```

### Phase 4: Scale Infrastructure (When Needed)

**Trigger conditions for adding Redis/PostgreSQL**:
- Multiple ECS instances needed (horizontal scaling)
- Session persistence required across deploys
- Financial audit requirements for tabs/refunds

```
Add Redis ($13-88/month):
- Migrate sessions to Redis
- Keep Moka as L1 cache

Add PostgreSQL ($12-145/month):
- Migrate tabs to PostgreSQL
- Migrate refund ledger
- Add audit logging
```

---

## 7. Creative Solutions

### 7.1 Solving Dynamic Pricing Without Escrow Contracts

**Problem**: EIP-3009 signatures are immutable - amount cannot be changed after signing.

**Solution: "Authorize Max, Settle Max, Refund Difference"**

```
1. Client authorizes MAX expected cost (e.g., $1.00 for AI inference)
2. Server executes request, tracks ACTUAL cost (e.g., $0.23)
3. Server settles FULL $1.00 on-chain (signature is valid)
4. Server records refund: $0.77 owed to client
5. Monthly: Server batches all refunds into single transaction

Trust assumption: Server must honor refunds
Mitigation: Transparent refund ledger, reputation system
```

**Why this works**:
- No smart contract changes needed
- Uses existing EIP-3009 infrastructure
- Batching saves gas (one refund tx for many users)
- Users see actual cost in response immediately

### 7.2 HTTP Message Signatures for Deferred Payments

**Based on Cloudflare's proposal** (RFC 9421 HTTP Message Signatures):

```rust
use base64::{Engine as _, engine::general_purpose::STANDARD as b64};

/// HTTP Message Signature components (RFC 9421)
#[derive(Debug, Clone)]
pub struct HttpMessageSignature {
    /// Signature value (base64)
    pub signature: String,
    /// Signature input string (defines what was signed)
    pub signature_input: String,
    /// Key identifier for verification
    pub key_id: String,
}

impl HttpMessageSignature {
    /// Verify signature for deferred payment identity
    pub async fn verify(
        &self,
        method: &str,
        path: &str,
        headers: &HashMap<String, String>,
    ) -> Result<MixedAddress, SignatureError> {
        // Parse signature-input to determine covered components
        // Reconstruct signature base
        // Verify using key_id to look up public key
        // Return wallet address if valid
        todo!("Implement RFC 9421 verification")
    }
}
```

### 7.3 Nonce-Based Concurrent Authorizations

**EIP-3009's Secret Weapon**: Random `bytes32` nonces

```rust
/// Generate concurrent payment authorizations
/// EIP-3009's random nonces allow unlimited parallel authorizations!
pub fn generate_subscription_authorizations(
    subscriber: EvmAddress,
    payee: EvmAddress,
    amount_per_period: TokenAmount,
    total_periods: u32,
    start_time: u64,
    period_seconds: u64,
) -> Vec<ExactEvmPayloadAuthorization> {
    (0..total_periods)
        .map(|period| {
            let valid_after = start_time + (period as u64 * period_seconds);
            let valid_before = valid_after + period_seconds;

            ExactEvmPayloadAuthorization {
                from: subscriber,
                to: payee,
                value: amount_per_period,
                valid_after: UnixTimestamp::from(valid_after),
                valid_before: UnixTimestamp::from(valid_before),
                // Each period gets unique random nonce - no conflicts!
                nonce: HexEncodedNonce(rand::random()),
            }
        })
        .collect()
}
```

### 7.4 Streaming Payments Future Architecture

**For future Superfluid integration**:

```rust
/// Future streaming scheme (requires Superfluid contracts)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingSchemePayload {
    /// Superfluid stream ID (if existing)
    pub stream_id: Option<String>,
    /// Flow rate in tokens per second (wei/sec for 6 decimal tokens)
    pub flow_rate: String,
    /// Super Token address (wrapped stablecoin)
    pub super_token: MixedAddress,
    /// Duration in seconds (0 = indefinite)
    pub duration: u64,
}

// Superfluid integration would require:
// 1. Deploying Super Token wrappers for USDC on each chain
// 2. Integrating Superfluid SDK
// 3. Managing stream lifecycle (create/update/delete)
// This is Phase 5+ work - defer until streaming demand materializes
```

---

## 8. Performance Analysis

### 8.1 Latency Impact by Feature

| Feature | Current | With Feature | Delta |
|---------|---------|--------------|-------|
| **No Session** | 200-500ms (full verify+settle) | N/A | baseline |
| **With Session (Moka)** | N/A | ~5 microseconds | **-99.99%** |
| **With Session (Redis)** | N/A | ~1-5ms | -99% |
| **Deferred Tab Charge** | N/A | ~100 microseconds | minimal |
| **Upto Settlement** | 200-500ms | 200-500ms + 50ms | +10% |

### 8.2 Memory Usage

```
Sessions (Moka):
- 10,000 sessions: ~10 MB
- 100,000 sessions: ~100 MB
- Automatic eviction when capacity reached

Deferred Tabs (In-Memory):
- 1,000 active tabs: ~1 MB
- With 100 charges each: ~10 MB

Refund Ledger (In-Memory):
- 10,000 pending refunds: ~5 MB
- File persistence: ~500 KB JSON
```

### 8.3 Throughput Estimates

```
Current (stateless):
- /settle: ~10 TPS (limited by RPC latency)
- /verify: ~50 TPS (no on-chain call)

With Sessions:
- Repeat access: ~100,000 TPS (Moka lookup)
- New sessions: ~10 TPS (still need initial settle)

With Deferred Tabs:
- Tab charges: ~50,000 TPS (in-memory)
- Tab settlements: ~10 TPS (on-chain)
```

---

## 9. Security Considerations

### 9.1 Session Security

```rust
/// Session security configuration
pub struct SessionSecurityConfig {
    /// JWT signing key (MUST be 256+ bits)
    /// Store in AWS Secrets Manager, NOT env vars
    pub jwt_secret: Vec<u8>,

    /// Different secrets for mainnet vs testnet
    /// Prevents cross-environment session reuse
    pub environment: Environment,

    /// Maximum session TTL
    pub max_ttl: Duration,

    /// Require session to be bound to specific resource
    pub resource_binding: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Environment {
    Mainnet,
    Testnet,
}
```

### 9.2 Deferred Tab Security

```
1. Tab ID: UUID v4 (cryptographically random, unpredictable)
2. Payer verification: Must match tab owner for all charges
3. Rate limiting: Max tabs per wallet, max charges per tab
4. Amount limits: Max total tab value before forced settlement
5. Age limits: Auto-settle tabs older than X days
```

### 9.3 Refund Integrity

```
1. actual_cost MUST be <= max_cost (server cannot overcharge)
2. Refund records MUST be append-only (no modification)
3. File persistence for crash recovery
4. Checksums for refund ledger integrity
5. Alert on refund processing failures
```

---

## 10. Code Architecture

### 10.1 New Module Structure

```
src/
├── lib.rs                  # Add: mod session, mod tabs, mod refunds
├── types.rs                # Add: new scheme payloads
├── session/
│   ├── mod.rs              # Session module exports
│   ├── claims.rs           # X402SessionClaims
│   ├── manager.rs          # SessionManager (JWT sign/verify)
│   └── store/
│       ├── mod.rs          # SessionStore trait
│       ├── moka.rs         # MokaSessionStore
│       └── redis.rs        # RedisSessionStore (future)
├── tabs/
│   ├── mod.rs              # Tab module exports
│   ├── types.rs            # DeferredTab, TabCharge
│   └── store/
│       ├── mod.rs          # TabStore trait
│       ├── memory.rs       # InMemoryTabStore
│       └── postgres.rs     # PostgresTabStore (future)
├── refunds/
│   ├── mod.rs              # Refund module exports
│   ├── types.rs            # RefundEntry, RefundBatch
│   ├── ledger.rs           # RefundLedger (in-memory + file)
│   └── processor.rs        # Background refund processor
└── handlers.rs             # Add: session, tab, subscription endpoints
```

### 10.2 Updated Scheme Enum

```rust
/// Payment schemes supported by x402
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    /// Standard exact amount transfer (EIP-3009)
    Exact,
    /// Fully Homomorphic Encryption transfer (ERC7984)
    #[serde(rename = "fhe-transfer")]
    FheTransfer,
    /// Deferred payment - accumulate charges, settle later
    Deferred,
    /// Dynamic pricing - authorize max, charge actual, refund difference
    Upto,
    /// Recurring subscription with pre-signed authorizations
    Subscription,
    // Future: Streaming (requires Superfluid)
    // Streaming,
}
```

### 10.3 Updated Facilitator Trait

```rust
/// Extended facilitator trait with v2 features
pub trait FacilitatorV2: Facilitator {
    /// Issue session token after successful settlement
    fn issue_session(
        &self,
        settle_response: &SettleResponse,
        resource: &Url,
        ttl: Duration,
    ) -> Result<String, Self::Error>;

    /// Validate session token
    fn validate_session(
        &self,
        token: &str,
    ) -> Result<X402SessionClaims, Self::Error>;

    /// Create deferred payment tab
    fn create_tab(
        &self,
        payer: MixedAddress,
        network: Network,
    ) -> impl Future<Output = Result<String, Self::Error>> + Send;

    /// Add charge to existing tab
    fn charge_tab(
        &self,
        tab_id: &str,
        amount: TokenAmount,
        description: String,
    ) -> impl Future<Output = Result<TokenAmount, Self::Error>> + Send;

    /// Settle and close tab
    fn settle_tab(
        &self,
        tab_id: &str,
        authorization: ExactEvmPayload,
    ) -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send;
}
```

---

## 11. Decision Matrix

### 11.1 What to Implement Now

| Feature | Priority | Complexity | Infrastructure | Implement? |
|---------|----------|------------|----------------|------------|
| Sessions (Moka) | HIGH | LOW | None | **YES** |
| Deferred Tabs (Memory) | HIGH | MEDIUM | None | **YES** |
| Upto + Refunds | HIGH | MEDIUM | None | **YES** |
| Subscriptions | MEDIUM | MEDIUM | None | Phase 3 |
| Redis Sessions | LOW | LOW | Redis | When scaling |
| PostgreSQL Tabs | LOW | MEDIUM | PostgreSQL | When audit needed |
| Streaming | LOW | HIGH | Superfluid | Future |

### 11.2 Configuration Questions

Before implementation, decide:

1. **Session TTL**:
   - [ ] 15 minutes (high security)
   - [ ] 1 hour (balanced) - **recommended**
   - [ ] 24 hours (convenience)

2. **Tab Settlement Trigger**:
   - [ ] Amount threshold ($10)
   - [ ] Time-based (daily/weekly)
   - [ ] Both (whichever first) - **recommended**

3. **Refund Processing**:
   - [ ] Real-time (expensive gas)
   - [ ] Daily batch
   - [ ] Monthly batch - **recommended**

4. **Subscription Retry Policy**:
   - [ ] Immediate cancellation
   - [ ] 3-day grace period - **recommended**
   - [ ] 7-day grace period

---

## Appendix A: Sources

### x402 Protocol
- [x402 V2 Launch Announcement](https://www.x402.org/writing/x402-v2-launch)
- [x402 Whitepaper](https://www.x402.org/x402-whitepaper.pdf)
- [Cloudflare x402 Blog](https://blog.cloudflare.com/x402/)

### Authentication Standards
- [EIP-4361: Sign-In with Ethereum](https://eips.ethereum.org/EIPS/eip-4361)
- [CAIP-122: Sign in With X](https://chainagnostic.org/CAIPs/caip-122)

### Payment Standards
- [EIP-3009: Transfer With Authorization](https://eips.ethereum.org/EIPS/eip-3009)
- [Superfluid Protocol](https://superfluid.org/)

### Rust Libraries
- [Moka Cache](https://github.com/moka-rs/moka)
- [jsonwebtoken](https://github.com/Keats/jsonwebtoken)

---

## Appendix B: Estimated Costs

### Phase 1-3 (Zero Infrastructure)
| Component | Monthly Cost |
|-----------|--------------|
| Compute (existing ECS) | $0 additional |
| Memory (sessions/tabs) | $0 (in-process) |
| Storage (refund ledger) | $0 (EBS included) |
| **Total** | **$0** |

### Phase 4+ (With Infrastructure)
| Component | Monthly Cost |
|-----------|--------------|
| Redis (ElastiCache t3.micro) | $13 |
| PostgreSQL (RDS t3.micro) | $12 |
| **Total** | **$25** |

### Production Scale
| Component | Monthly Cost |
|-----------|--------------|
| Redis (ElastiCache m6g.large) | $88 |
| PostgreSQL (RDS m6g.large) | $145 |
| **Total** | **$233** |

---

*Document Complete*
*Ready for implementation review and phase 1 kickoff*
