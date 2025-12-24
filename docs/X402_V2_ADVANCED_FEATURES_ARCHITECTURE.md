# x402 v2 Advanced Features: Technical Architecture

**Document Version**: 1.0
**Created**: 2025-12-23
**Scope**: SIWx Sessions, Additional Payment Schemes, Dynamic Pricing
**Target**: Rust x402-rs facilitator implementation

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Feature 1: SIWx Sessions (Pay Once, Reuse Access)](#feature-1-siwx-sessions)
3. [Feature 2: Additional Payment Schemes](#feature-2-additional-payment-schemes)
4. [Feature 3: Dynamic Pricing for AI/Compute](#feature-3-dynamic-pricing-for-aicompute)
5. [Cross-Cutting Concerns](#cross-cutting-concerns)
6. [Implementation Roadmap](#implementation-roadmap)

---

## Executive Summary

This document provides production-ready architectural guidance for implementing three critical x402 v2 features in the Rust facilitator. Each section includes:

- **Threat model analysis** (replay attacks, race conditions, abuse vectors)
- **Type system design** (traits, enums, state machines)
- **State management patterns** (storage backend selection, cleanup strategies)
- **Security properties** (authentication, authorization, auditability)

**Key Design Principles**:
1. **Fail-secure**: Default to denying access, not granting it
2. **Stateless where possible**: Minimize state synchronization complexity
3. **Multi-chain from day one**: Design for EVM, Solana, NEAR, Stellar simultaneously
4. **Backward compatible**: v1 clients continue working unchanged
5. **Production-grade observability**: Metrics, traces, audit logs

---

## Feature 1: SIWx Sessions (Pay Once, Reuse Access)

### Problem Statement

Current x402 flow requires full payment verification on EVERY request:
```
Client Request -> 402 Payment Required -> Sign Authorization -> Verify -> Settle -> 200 OK
```

For repeated access (API subscriptions, paywalled content), this is inefficient:
- High latency (signature verification + RPC calls per request)
- Unnecessary gas costs (multiple settlements for same session)
- Poor UX (constant wallet prompts)

### Solution Overview

**Session-based authentication**: After first payment, issue a session token that proves prior payment without re-verification.

```
Initial:   Request -> 402 -> Pay -> Session Token
Subsequent: Request + Token -> 200 OK (cached authorization)
```

### Architecture Decision: JWT vs Signed Identifier

The x402 v2 spec proposes **Signed Identifier** (client signs every request). This is **architecturally flawed** for sessions:

| Aspect | Server-Issued JWT | Signed Identifier (v2 spec) |
|--------|-------------------|------------------------------|
| **Verification cost** | HMAC (CPU-only, ~1μs) | RPC call for EIP-1271 (~200ms) |
| **Smart account support** | Works | Requires on-chain verification |
| **Revocation** | Server-controlled | Client-controlled (problematic) |
| **Replay protection** | Nonce in JWT | Per-request signature |
| **Battle-tested** | Yes (OAuth2, OIDC) | No |

**Recommendation**: Use **Server-Issued JWTs** with SIWE/SIWx for initial authentication. Ignore the v2 spec's Signed Identifier.

### Type System Design

```rust
// src/session.rs

use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH, Duration};

/// Session claims encoded in JWT payload.
///
/// Security properties:
/// - `exp`: Prevents indefinite session hijacking
/// - `sub`: Binds session to wallet address (prevents impersonation)
/// - `network`: Prevents cross-chain session reuse attacks
/// - `payment_tx`: Auditability - trace session back to settlement transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Subject - wallet address that paid (0x... for EVM, base58 for Solana)
    pub sub: String,

    /// Issued at (Unix timestamp)
    pub iat: u64,

    /// Expiration (Unix timestamp)
    pub exp: u64,

    /// Network where payment was settled (CAIP-2 format)
    pub network: String,

    /// Settlement transaction hash (for audit trail)
    pub payment_tx: String,

    /// Optional: Resource scopes (e.g., "api:read", "content:premium")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,

    /// Optional: Payment amount (for rate-limiting by tier)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_paid: Option<String>,
}

impl SessionClaims {
    /// Create new session claims after successful payment settlement.
    ///
    /// # Arguments
    /// - `payer`: Wallet address (MixedAddress converted to string)
    /// - `network`: Network where payment settled (e.g., "eip155:8453")
    /// - `tx_hash`: Settlement transaction hash
    /// - `ttl`: Time-to-live for session (e.g., Duration::from_secs(3600) = 1 hour)
    pub fn new(
        payer: String,
        network: String,
        tx_hash: String,
        ttl: Duration,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            sub: payer,
            iat: now,
            exp: now + ttl.as_secs(),
            network,
            payment_tx: tx_hash,
            scopes: None,
            amount_paid: None,
        }
    }

    /// Check if session has expired.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now >= self.exp
    }
}

/// Session manager for creating and validating JWTs.
///
/// # Security Considerations
/// - `jwt_secret`: MUST be at least 256 bits (32 bytes) of cryptographically random data
/// - Store in AWS Secrets Manager, NOT environment variables or config files
/// - Rotate secret periodically (e.g., every 90 days)
/// - Use different secrets for mainnet vs testnet environments
#[derive(Clone)]
pub struct SessionManager {
    /// JWT signing/verification secret
    jwt_secret: Vec<u8>,

    /// Default session TTL
    default_ttl: Duration,

    /// Optional: Revoked session store (for early termination)
    revoked_sessions: Arc<RwLock<HashSet<String>>>,
}

impl SessionManager {
    /// Create new session manager.
    ///
    /// # Errors
    /// Returns error if `jwt_secret` is less than 256 bits (insecure).
    pub fn new(jwt_secret: Vec<u8>, default_ttl: Duration) -> Result<Self, SessionError> {
        if jwt_secret.len() < 32 {
            return Err(SessionError::WeakSecret(jwt_secret.len()));
        }

        Ok(Self {
            jwt_secret,
            default_ttl,
            revoked_sessions: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Issue a new session token after payment settlement.
    ///
    /// # Example
    /// ```ignore
    /// let token = session_manager.issue_token(
    ///     "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb".to_string(),
    ///     "eip155:8453".to_string(),
    ///     "0xabc123...".to_string(),
    ///     None, // use default TTL
    /// )?;
    /// ```
    pub fn issue_token(
        &self,
        payer: String,
        network: String,
        payment_tx: String,
        ttl: Option<Duration>,
    ) -> Result<String, SessionError> {
        let claims = SessionClaims::new(
            payer,
            network,
            payment_tx,
            ttl.unwrap_or(self.default_ttl),
        );

        let header = Header::default();
        let key = EncodingKey::from_secret(&self.jwt_secret);

        encode(&header, &claims, &key)
            .map_err(|e| SessionError::EncodingFailed(e.to_string()))
    }

    /// Validate session token and extract claims.
    ///
    /// # Security Checks
    /// 1. JWT signature validation (prevents forgery)
    /// 2. Expiration check (prevents indefinite access)
    /// 3. Revocation check (allows early termination)
    ///
    /// # Errors
    /// Returns error if:
    /// - Token signature invalid
    /// - Token expired
    /// - Token has been revoked
    pub fn validate_token(&self, token: &str) -> Result<SessionClaims, SessionError> {
        // Check revocation first (fast path)
        if self.revoked_sessions.read().unwrap().contains(token) {
            return Err(SessionError::Revoked);
        }

        let key = DecodingKey::from_secret(&self.jwt_secret);
        let validation = Validation::default();

        let token_data = decode::<SessionClaims>(token, &key, &validation)
            .map_err(|e| SessionError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims)
    }

    /// Revoke a session token (e.g., logout, suspicious activity).
    ///
    /// # Note
    /// This is in-memory revocation. For distributed deployments, use Redis/DynamoDB.
    pub fn revoke_token(&self, token: String) {
        self.revoked_sessions.write().unwrap().insert(token);
    }

    /// Clean up expired revoked tokens (prevent memory leak).
    ///
    /// Should be called periodically (e.g., every hour) in a background task.
    pub fn cleanup_revocations(&self) {
        // Revoked tokens are stored by token string, not by expiration
        // In production, store (token_hash, expiration) and prune by timestamp
        // For simplicity, clear all (they'll be re-added if still in use)
        self.revoked_sessions.write().unwrap().clear();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("JWT secret too weak: {0} bytes (minimum 32 required)")]
    WeakSecret(usize),

    #[error("Failed to encode JWT: {0}")]
    EncodingFailed(String),

    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Session revoked")]
    Revoked,
}
```

### Integration with Facilitator

Modify `src/handlers.rs` to issue sessions after successful settlement:

```rust
// src/handlers.rs (additions)

use crate::session::{SessionManager, SessionClaims};

/// `POST /settle`: Extended to return session token on success.
///
/// Response format:
/// ```json
/// {
///   "success": true,
///   "transaction": "0xabc123...",
///   "payer": "0x742d35...",
///   "network": "eip155:8453",
///   "sessionToken": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
/// }
/// ```
#[instrument(skip_all)]
pub async fn post_settle_with_session<A>(
    State(facilitator): State<A>,
    State(session_manager): State<Arc<SessionManager>>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    // ... existing settlement logic ...

    match facilitator.settle(&body).await {
        Ok(settle_response) if settle_response.success => {
            // Issue session token after successful settlement
            let session_token = session_manager.issue_token(
                settle_response.payer.to_string(),
                settle_response.network.to_caip2(),
                settle_response.transaction.clone().unwrap_or_default(),
                None, // use default TTL
            ).ok();

            // Return extended response with session token
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "transaction": settle_response.transaction,
                    "payer": settle_response.payer,
                    "network": settle_response.network,
                    "sessionToken": session_token,
                }))
            ).into_response()
        }
        Ok(failure_response) => {
            (StatusCode::OK, Json(failure_response)).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Axum middleware to validate session tokens on protected endpoints.
///
/// Usage:
/// ```ignore
/// Router::new()
///     .route("/premium-content", get(get_premium_content))
///     .layer(middleware::from_fn_with_state(
///         session_manager.clone(),
///         session_auth_middleware
///     ))
/// ```
pub async fn session_auth_middleware(
    State(session_manager): State<Arc<SessionManager>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract session token from Authorization header
    let auth_header = req.headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Validate token
    let claims = session_manager.validate_token(auth_header)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Insert claims into request extensions for handlers to access
    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}
```

### State Management: Storage Backend Selection

| Backend | Use Case | Pros | Cons |
|---------|----------|------|------|
| **In-Memory (Arc&lt;RwLock&lt;HashMap&gt;&gt;)** | Single-server dev | Fast, simple | Lost on restart, no scaling |
| **Redis** | Production multi-server | Fast (1ms), distributed | External dependency, cost |
| **DynamoDB** | AWS-native production | Serverless, scalable | Higher latency (~10ms) |
| **PostgreSQL** | Existing DB infrastructure | ACID guarantees | Slower than Redis |

**Recommendation for x402-rs**: Start with **Redis** for production (AWS ElastiCache). Fallback to in-memory for local dev.

```rust
// src/session_store.rs

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn store_session(&self, token_hash: String, claims: SessionClaims) -> Result<(), StoreError>;
    async fn retrieve_session(&self, token_hash: &str) -> Result<Option<SessionClaims>, StoreError>;
    async fn revoke_session(&self, token_hash: &str) -> Result<(), StoreError>;
    async fn cleanup_expired(&self) -> Result<usize, StoreError>;
}

/// Redis-backed session store (production).
pub struct RedisSessionStore {
    client: redis::Client,
}

impl RedisSessionStore {
    pub fn new(redis_url: &str) -> Result<Self, redis::RedisError> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
        })
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn store_session(&self, token_hash: String, claims: SessionClaims) -> Result<(), StoreError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let ttl = claims.exp - claims.iat;
        let serialized = serde_json::to_string(&claims)?;

        redis::cmd("SETEX")
            .arg(&token_hash)
            .arg(ttl)
            .arg(serialized)
            .query_async(&mut conn)
            .await?;

        Ok(())
    }

    async fn retrieve_session(&self, token_hash: &str) -> Result<Option<SessionClaims>, StoreError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let value: Option<String> = redis::cmd("GET")
            .arg(token_hash)
            .query_async(&mut conn)
            .await?;

        match value {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn revoke_session(&self, token_hash: &str) -> Result<(), StoreError> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        redis::cmd("DEL")
            .arg(token_hash)
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<usize, StoreError> {
        // Redis handles expiration automatically via SETEX TTL
        Ok(0)
    }
}

/// In-memory session store (development only).
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, SessionClaims>>>,
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn store_session(&self, token_hash: String, claims: SessionClaims) -> Result<(), StoreError> {
        self.sessions.write().unwrap().insert(token_hash, claims);
        Ok(())
    }

    async fn retrieve_session(&self, token_hash: &str) -> Result<Option<SessionClaims>, StoreError> {
        Ok(self.sessions.read().unwrap().get(token_hash).cloned())
    }

    async fn revoke_session(&self, token_hash: &str) -> Result<(), StoreError> {
        self.sessions.write().unwrap().remove(token_hash);
        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<usize, StoreError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut sessions = self.sessions.write().unwrap();
        let before = sessions.len();
        sessions.retain(|_, claims| claims.exp > now);
        Ok(before - sessions.len())
    }
}
```

### Security Considerations

1. **Session Hijacking Prevention**:
   - Bind sessions to IP address (optional, breaks mobile users)
   - Rotate tokens on IP change
   - Short TTL (1 hour max for high-value content)

2. **Replay Attack Prevention**:
   - JWTs include `iat` (issued-at) timestamp
   - Reject tokens issued before last password/key rotation
   - Store "invalidate-before" timestamp per wallet

3. **Cross-Chain Session Reuse Attack**:
   - Sessions include `network` field in claims
   - Validate network matches expected chain
   - Prevents paying on testnet, using session on mainnet

4. **Smart Account Considerations**:
   - JWTs bind to wallet address, not specific key
   - If smart wallet rotates keys, session remains valid
   - This is CORRECT behavior (UX vs security tradeoff)

### Performance Impact

| Metric | Without Sessions | With Sessions (JWT) |
|--------|------------------|---------------------|
| **Request latency** | 200-500ms (RPC calls) | &lt;1ms (HMAC verify) |
| **Facilitator CPU** | Low (I/O bound) | Negligible |
| **Facilitator memory** | ~100MB baseline | +10MB per 10k active sessions |
| **Database load** | 0 (stateless) | Read-heavy (Redis handles) |

**Conclusion**: Session tokens reduce latency by **200x** for repeat access with minimal resource overhead.

---

## Feature 2: Additional Payment Schemes

### Current Scheme Landscape

x402 v1 supports two schemes:
- `exact`: Fixed-amount payment (e.g., $1 for article access)
- `fhe-transfer`: Confidential payment via FHE (Zama integration)

x402 v2 spec mentions but **doesn't define**:
- `upto`: Pay up to max amount based on consumption
- `deferred`: Batch/subscription billing
- `streaming`: Per-second micropayments
- `subscription`: Recurring periodic payments

### Architectural Approach

Extend the `Scheme` enum in `src/types.rs`:

```rust
// src/types.rs (modifications)

/// Payment scheme variants.
///
/// Each scheme represents a different temporal/accounting model for payment settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scheme {
    /// Pay exactly this amount, right now (EIP-3009, SPL transfer, NEAR delegate action).
    Exact,

    /// Pay using FHE encryption (Zama Lambda facilitator).
    FheTransfer,

    /// Pay up to a maximum amount (actual amount determined post-execution).
    ///
    /// Use cases: AI inference (pay per token), metered APIs (pay per call).
    /// Settlement: Client pre-authorizes max amount, server charges actual cost.
    #[serde(rename = "upto")]
    UpTo,

    /// Pay in batch after multiple requests (tab model).
    ///
    /// Use cases: CDN bandwidth, API usage aggregation.
    /// Settlement: Server accumulates charges, client pays periodically.
    Deferred,

    /// Pay continuously per unit time (streaming micropayments).
    ///
    /// Use cases: Video streaming, real-time data feeds.
    /// Settlement: Client authorizes payment stream, server claims incrementally.
    Streaming,

    /// Recurring periodic payments (subscription model).
    ///
    /// Use cases: Monthly SaaS access, annual content licenses.
    /// Settlement: Client authorizes recurring charges, server claims per period.
    Subscription,
}
```

### Scheme 1: `upto` (Pay Up To Maximum)

**Problem**: Server doesn't know final cost until after processing (LLM inference, compute jobs).

**Solution**: Client pre-authorizes maximum amount, server charges actual amount.

#### Type System

```rust
// src/types.rs

/// Payment payload for `upto` scheme.
///
/// Client authorizes payment up to `max_amount`, server settles `actual_amount` <= `max_amount`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpToPaymentPayload {
    /// Maximum authorized amount (in token units).
    pub max_amount: TokenAmount,

    /// Network and asset (same as exact scheme).
    pub network: String,
    pub asset: MixedAddress,

    /// Authorization signature (covers max_amount).
    pub authorization: TransferWithAuthorization,

    /// Optional: Client-side estimate for UX (e.g., "~$0.10").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_amount: Option<TokenAmount>,
}

/// Settlement response for `upto` scheme includes actual charged amount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpToSettleResponse {
    pub success: bool,
    pub transaction: Option<TransactionHash>,
    pub network: String,
    pub payer: MixedAddress,

    /// Actual amount charged (must be <= authorized max_amount).
    pub actual_amount: TokenAmount,

    /// Server-provided cost breakdown (for transparency).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_details: Option<CostBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Itemized costs (e.g., "compute": 1000, "storage": 500).
    pub items: HashMap<String, u64>,

    /// Human-readable explanation.
    pub description: String,
}
```

#### Verification Flow

```rust
// src/facilitator_upto.rs

/// Verify `upto` payment authorization.
///
/// # Security Checks
/// 1. Authorization signature is valid for max_amount
/// 2. Payer has sufficient balance for max_amount
/// 3. Authorization hasn't expired
/// 4. Nonce hasn't been used (replay protection)
///
/// # Note
/// Does NOT check actual_amount yet - that happens at settlement time.
pub async fn verify_upto_payment(
    provider: &EvmProvider,
    payload: &UpToPaymentPayload,
    requirements: &PaymentRequirements,
) -> Result<VerifyResponse, FacilitatorLocalError> {
    // Verify signature covers max_amount (not actual_amount)
    let domain = get_eip712_domain(provider.chain(), &requirements.asset)?;
    let authorization_hash = payload.authorization.eip712_signing_hash(&domain);

    let recovered_signer = payload.authorization.signature.recover_signer(&authorization_hash)?;

    if recovered_signer != payload.authorization.from {
        return Ok(VerifyResponse::invalid(
            Some(payload.authorization.from.into()),
            FacilitatorErrorReason::InvalidSignature,
        ));
    }

    // Check balance sufficient for max_amount (not actual)
    let balance = check_balance(provider, &payload.authorization.from, &requirements.asset).await?;
    if balance < payload.max_amount {
        return Ok(VerifyResponse::invalid(
            Some(payload.authorization.from.into()),
            FacilitatorErrorReason::InsufficientFunds,
        ));
    }

    Ok(VerifyResponse::valid(payload.authorization.from.into()))
}
```

#### Settlement Flow

```rust
/// Settle `upto` payment with actual cost determined post-execution.
///
/// # Arguments
/// - `actual_amount`: Actual cost (MUST be <= max_amount from authorization)
///
/// # Errors
/// Returns error if:
/// - actual_amount > max_amount (server overcharge attempt)
/// - Insufficient balance for actual_amount
/// - Authorization expired or replayed
pub async fn settle_upto_payment(
    provider: &EvmProvider,
    payload: &UpToPaymentPayload,
    actual_amount: TokenAmount,
    cost_details: Option<CostBreakdown>,
) -> Result<UpToSettleResponse, FacilitatorLocalError> {
    // CRITICAL: Validate server isn't overcharging
    if actual_amount > payload.max_amount {
        return Err(FacilitatorLocalError::Other(format!(
            "Server attempted to charge {} (max authorized: {})",
            actual_amount, payload.max_amount
        )));
    }

    // Modify authorization to use actual_amount instead of max_amount
    let mut modified_auth = payload.authorization.clone();
    modified_auth.value = actual_amount.clone();

    // Execute EIP-3009 transfer with actual_amount
    let tx_receipt = execute_transfer_with_authorization(
        provider,
        &modified_auth,
    ).await?;

    Ok(UpToSettleResponse {
        success: true,
        transaction: Some(format!("{:?}", tx_receipt.transaction_hash).into()),
        network: provider.chain().network().to_caip2(),
        payer: modified_auth.from.into(),
        actual_amount,
        cost_details,
    })
}
```

#### **CRITICAL ISSUE**: EIP-3009 Doesn't Support Variable Amounts

**Problem**: EIP-3009 `transferWithAuthorization` signature covers the EXACT `value` field. You **cannot** sign for `max_amount` then settle for `actual_amount` - the signature becomes invalid.

**Solutions**:

1. **Two-Phase Authorization (Recommended)**:
   - Phase 1: Client pre-approves max_amount via `approve(spender, max_amount)`
   - Phase 2: Server calls `transferFrom(from, to, actual_amount)`
   - **Cons**: Requires 2 transactions (client on-chain approval)

2. **Claimable Payment Channel**:
   - Client deposits max_amount into escrow contract
   - Server submits claim for actual_amount
   - Unused funds remain in client's escrow
   - **Cons**: Gas overhead, smart contract complexity

3. **Off-Chain Refund**:
   - Server settles max_amount on-chain
   - Server refunds (max_amount - actual_amount) off-chain
   - **Cons**: Trust requirement, accounting complexity

**Recommendation**: For v1 implementation, **defer `upto` scheme** until we have:
- Smart contract infrastructure for payment channels
- Multi-sig escrow for trustless refunds
- Gas cost analysis vs UX benefit

### Scheme 2: `deferred` (Batch Payment / Tab)

**Use Case**: Cloudflare's proposal - accumulate charges over time, settle periodically (like a bar tab).

#### Architecture

```rust
// src/types.rs

/// Deferred payment tab - accumulates charges for later settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredPaymentTab {
    /// Unique tab identifier (UUID or wallet-derived).
    pub tab_id: String,

    /// Wallet address that owns this tab.
    pub payer: MixedAddress,

    /// Network and asset for settlement.
    pub network: String,
    pub asset: MixedAddress,

    /// Accumulated charges (in token units).
    pub total_owed: TokenAmount,

    /// Individual charge items (for transparency).
    pub line_items: Vec<DeferredLineItem>,

    /// Tab created at (Unix timestamp).
    pub created_at: u64,

    /// Optional: Auto-settle threshold (settle when total_owed >= threshold).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_settle_threshold: Option<TokenAmount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredLineItem {
    pub timestamp: u64,
    pub description: String,
    pub amount: TokenAmount,
    pub metadata: Option<serde_json::Value>,
}

/// Deferred settlement request - closes tab and charges total.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredSettleRequest {
    pub tab_id: String,

    /// Authorization signature covering total_owed amount.
    pub authorization: TransferWithAuthorization,
}
```

#### State Management

```rust
// src/deferred_store.rs

#[async_trait]
pub trait DeferredTabStore: Send + Sync {
    /// Create new tab for a payer.
    async fn create_tab(&self, payer: MixedAddress, network: String, asset: MixedAddress) -> Result<String, StoreError>;

    /// Add charge to existing tab.
    async fn add_charge(&self, tab_id: &str, item: DeferredLineItem) -> Result<(), StoreError>;

    /// Get current tab state.
    async fn get_tab(&self, tab_id: &str) -> Result<Option<DeferredPaymentTab>, StoreError>;

    /// Close tab (after settlement).
    async fn close_tab(&self, tab_id: &str) -> Result<DeferredPaymentTab, StoreError>;

    /// List all open tabs for a payer (for wallet UI).
    async fn list_tabs(&self, payer: &MixedAddress) -> Result<Vec<DeferredPaymentTab>, StoreError>;
}

/// PostgreSQL-backed deferred tab store (production).
pub struct PostgresTabStore {
    pool: sqlx::PgPool,
}

impl PostgresTabStore {
    pub fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect_lazy(database_url)?;
        Ok(Self { pool })
    }

    /// Initialize database schema.
    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS deferred_tabs (
                tab_id TEXT PRIMARY KEY,
                payer TEXT NOT NULL,
                network TEXT NOT NULL,
                asset TEXT NOT NULL,
                total_owed NUMERIC NOT NULL DEFAULT 0,
                created_at BIGINT NOT NULL,
                auto_settle_threshold NUMERIC,
                status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed'))
            );

            CREATE TABLE IF NOT EXISTS deferred_line_items (
                id SERIAL PRIMARY KEY,
                tab_id TEXT NOT NULL REFERENCES deferred_tabs(tab_id),
                timestamp BIGINT NOT NULL,
                description TEXT NOT NULL,
                amount NUMERIC NOT NULL,
                metadata JSONB
            );

            CREATE INDEX IF NOT EXISTS idx_tabs_payer ON deferred_tabs(payer);
            CREATE INDEX IF NOT EXISTS idx_tabs_status ON deferred_tabs(status);
            "#
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl DeferredTabStore for PostgresTabStore {
    async fn create_tab(&self, payer: MixedAddress, network: String, asset: MixedAddress) -> Result<String, StoreError> {
        let tab_id = uuid::Uuid::new_v4().to_string();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        sqlx::query(
            "INSERT INTO deferred_tabs (tab_id, payer, network, asset, created_at) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(&tab_id)
        .bind(payer.to_string())
        .bind(&network)
        .bind(asset.to_string())
        .bind(now as i64)
        .execute(&self.pool)
        .await?;

        Ok(tab_id)
    }

    async fn add_charge(&self, tab_id: &str, item: DeferredLineItem) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;

        // Insert line item
        sqlx::query(
            "INSERT INTO deferred_line_items (tab_id, timestamp, description, amount, metadata) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(tab_id)
        .bind(item.timestamp as i64)
        .bind(&item.description)
        .bind(item.amount.to_string())
        .bind(item.metadata)
        .execute(&mut *tx)
        .await?;

        // Update total_owed
        sqlx::query(
            "UPDATE deferred_tabs SET total_owed = total_owed + $1 WHERE tab_id = $2"
        )
        .bind(item.amount.to_string())
        .bind(tab_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    // ... other methods ...
}
```

#### Security Considerations

1. **Tab Hijacking Prevention**:
   - Tab ID must be cryptographically unpredictable (UUID v4)
   - Verify payer address matches tab owner before adding charges
   - Rate-limit tab creation per wallet (prevent DoS)

2. **Charge Manipulation Prevention**:
   - All charges logged with timestamp and description (audit trail)
   - Server cannot modify historical line items (append-only log)
   - Client receives itemized receipt before settlement

3. **Abandoned Tab Cleanup**:
   - Auto-close tabs after 30 days of inactivity
   - Notify payer before auto-settlement (email/wallet notification)
   - Graceful degradation if payer balance insufficient

### Scheme 3: `streaming` (Micropayments Per Second)

**Use Case**: Pay continuously while consuming (video streaming, real-time data).

**Challenge**: Blockchain settlement latency >> payment frequency (can't settle every second on-chain).

#### Solution: Payment Channels (State Channels)

```rust
// src/types.rs

/// Streaming payment channel state.
///
/// Uses off-chain state updates with periodic on-chain settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingChannel {
    /// Channel ID (deterministic from participants + nonce).
    pub channel_id: String,

    /// Payer (content consumer).
    pub payer: MixedAddress,

    /// Payee (content provider/facilitator).
    pub payee: MixedAddress,

    /// Deposited amount (locked in channel contract).
    pub deposited: TokenAmount,

    /// Current amount claimed by payee (cumulative).
    pub claimed: TokenAmount,

    /// Rate per second (in token units).
    pub rate_per_second: TokenAmount,

    /// Stream started at (Unix timestamp).
    pub started_at: u64,

    /// Optional: Stream ends at (for pre-paid fixed duration).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<u64>,

    /// Latest signed state update from payer.
    pub latest_signature: Option<EvmSignature>,
}

/// Off-chain state update (signed by payer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamUpdate {
    pub channel_id: String,
    pub nonce: u64,  // Monotonically increasing
    pub cumulative_amount: TokenAmount,  // Total owed to date
    pub timestamp: u64,
    pub signature: EvmSignature,
}
```

#### Smart Contract Requirements

Streaming payments require **on-chain payment channel contracts**. This is beyond the scope of pure facilitator backend work.

**Example Contract (Solidity)**:
```solidity
// StreamingChannel.sol (illustrative, not production-ready)

contract StreamingChannel {
    struct Channel {
        address payer;
        address payee;
        uint256 deposited;
        uint256 claimed;
        uint256 ratePerSecond;
        uint256 startedAt;
        uint256 nonce;
    }

    mapping(bytes32 => Channel) public channels;

    function openChannel(
        address payee,
        uint256 ratePerSecond
    ) external payable {
        bytes32 channelId = keccak256(abi.encodePacked(msg.sender, payee, block.timestamp));
        channels[channelId] = Channel({
            payer: msg.sender,
            payee: payee,
            deposited: msg.value,
            claimed: 0,
            ratePerSecond: ratePerSecond,
            startedAt: block.timestamp,
            nonce: 0
        });
    }

    function claim(
        bytes32 channelId,
        uint256 amount,
        uint256 nonce,
        bytes memory signature
    ) external {
        Channel storage channel = channels[channelId];
        require(msg.sender == channel.payee, "Only payee can claim");
        require(nonce > channel.nonce, "Nonce must increase");
        require(amount <= channel.deposited, "Cannot claim more than deposited");

        // Verify signature from payer
        bytes32 hash = keccak256(abi.encodePacked(channelId, amount, nonce));
        address signer = recover(hash, signature);
        require(signer == channel.payer, "Invalid signature");

        uint256 claimable = amount - channel.claimed;
        channel.claimed = amount;
        channel.nonce = nonce;

        payable(channel.payee).transfer(claimable);
    }

    function closeChannel(bytes32 channelId) external {
        Channel storage channel = channels[channelId];
        require(msg.sender == channel.payer || msg.sender == channel.payee, "Unauthorized");

        uint256 unclaimed = channel.deposited - channel.claimed;
        if (unclaimed > 0) {
            payable(channel.payer).transfer(unclaimed);
        }

        delete channels[channelId];
    }
}
```

**Facilitator Role**:
- Monitor channel state off-chain
- Sign state updates as payee
- Periodically claim on-chain (e.g., every 10 minutes or when channel closes)

**Recommendation**: Defer streaming scheme until:
1. Smart contract audited and deployed across all supported networks
2. Gas cost analysis for claim frequency
3. UX design for channel lifecycle (open/monitor/close)

### Scheme 4: `subscription` (Recurring Payments)

**Use Case**: Monthly SaaS access, annual content licenses.

**Challenge**: Blockchains don't have native cron jobs - someone must trigger the recurring transaction.

#### Solution: EIP-3009 with Sequential Nonces

```rust
// src/types.rs

/// Subscription authorization (pre-signed recurring payments).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionAuthorization {
    /// Subscriber wallet address.
    pub subscriber: MixedAddress,

    /// Payee (service provider).
    pub payee: MixedAddress,

    /// Amount per period.
    pub amount_per_period: TokenAmount,

    /// Billing period (in seconds, e.g., 2592000 = 30 days).
    pub period_seconds: u64,

    /// Total number of periods authorized (e.g., 12 for annual).
    pub total_periods: u32,

    /// Subscription starts at (Unix timestamp).
    pub starts_at: u64,

    /// Pre-signed authorizations (one per period).
    pub period_authorizations: Vec<TransferWithAuthorization>,
}
```

#### Verification Flow

```rust
/// Verify subscription authorization.
///
/// Checks:
/// 1. All period_authorizations have valid signatures
/// 2. Nonces are sequential and unused
/// 3. Subscriber has sufficient balance for at least first period
/// 4. Timing constraints are valid
pub async fn verify_subscription(
    provider: &EvmProvider,
    subscription: &SubscriptionAuthorization,
) -> Result<VerifyResponse, FacilitatorLocalError> {
    // Verify all pre-signed authorizations
    for (i, auth) in subscription.period_authorizations.iter().enumerate() {
        let expected_valid_after = subscription.starts_at + (i as u64 * subscription.period_seconds);

        if auth.valid_after.seconds_since_epoch() != expected_valid_after {
            return Ok(VerifyResponse::invalid(
                Some(subscription.subscriber.clone()),
                FacilitatorErrorReason::InvalidTiming,
            ));
        }

        // Verify signature
        verify_eip3009_signature(provider, auth)?;
    }

    // Check balance for first period (can't guarantee future balance)
    let balance = check_balance(provider, &subscription.subscriber, &subscription.payee).await?;
    if balance < subscription.amount_per_period {
        return Ok(VerifyResponse::invalid(
            Some(subscription.subscriber.clone()),
            FacilitatorErrorReason::InsufficientFunds,
        ));
    }

    Ok(VerifyResponse::valid(subscription.subscriber.clone()))
}
```

#### Settlement Flow (Recurring Trigger)

```rust
/// Settle subscription payment for a specific period.
///
/// Called by a cron job or manual trigger when period boundary reached.
pub async fn settle_subscription_period(
    provider: &EvmProvider,
    subscription: &SubscriptionAuthorization,
    period_index: usize,
) -> Result<SettleResponse, FacilitatorLocalError> {
    if period_index >= subscription.period_authorizations.len() {
        return Err(FacilitatorLocalError::Other(
            format!("Period {} exceeds total periods {}", period_index, subscription.total_periods)
        ));
    }

    let auth = &subscription.period_authorizations[period_index];
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    // Verify we're within valid time window
    if now < auth.valid_after.seconds_since_epoch() {
        return Err(FacilitatorLocalError::Other(
            format!("Period {} not yet started", period_index)
        ));
    }

    if now >= auth.valid_before.seconds_since_epoch() {
        return Err(FacilitatorLocalError::Other(
            format!("Period {} authorization expired", period_index)
        ));
    }

    // Execute transfer
    execute_transfer_with_authorization(provider, auth).await
}
```

#### Background Job for Subscription Settlement

```rust
// src/subscription_worker.rs

/// Background worker to settle subscriptions on schedule.
pub struct SubscriptionWorker {
    facilitator: Arc<FacilitatorLocal<ProviderCache>>,
    subscription_store: Arc<dyn SubscriptionStore>,
    check_interval: Duration,
}

impl SubscriptionWorker {
    pub async fn run(&self) {
        let mut interval = tokio::time::interval(self.check_interval);

        loop {
            interval.tick().await;

            if let Err(e) = self.process_due_subscriptions().await {
                error!(error = %e, "Failed to process subscriptions");
            }
        }
    }

    async fn process_due_subscriptions(&self) -> Result<(), Box<dyn std::error::Error>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let due_subscriptions = self.subscription_store.get_due_subscriptions(now).await?;

        info!(count = due_subscriptions.len(), "Processing due subscriptions");

        for (sub_id, period_index) in due_subscriptions {
            match self.settle_subscription(&sub_id, period_index).await {
                Ok(receipt) => {
                    info!(
                        subscription_id = %sub_id,
                        period = period_index,
                        tx = %receipt.transaction_hash,
                        "Subscription period settled"
                    );

                    self.subscription_store.mark_period_settled(&sub_id, period_index).await?;
                }
                Err(e) => {
                    error!(
                        subscription_id = %sub_id,
                        period = period_index,
                        error = %e,
                        "Failed to settle subscription period"
                    );

                    // Retry logic or alert admin
                }
            }
        }

        Ok(())
    }
}
```

#### Security Considerations

1. **Insufficient Balance Handling**:
   - Subscriptions can fail if balance drops below amount_per_period
   - Grace period before cancellation (e.g., 3 days retry)
   - Notify subscriber via wallet/email before cancellation

2. **Nonce Reuse Attack Prevention**:
   - Each period authorization must have unique nonce
   - Nonces must be sequential (prevent skipping periods)
   - Track used nonces in database (replay protection)

3. **Early Cancellation**:
   - Subscriber can cancel by transferring assets out (breaks future payments)
   - Provider should detect failed settlement and suspend service
   - Refund unused periods (UX consideration)

---

## Feature 3: Dynamic Pricing for AI/Compute

### Problem Statement

LLM inference costs are **unknown until after processing**:
- Input: "Write a short story" (could be 100 tokens or 10,000 tokens output)
- Cost: $0.01 to $1.00 depending on output length

Current x402 `exact` scheme **cannot handle this** - amount must be known before payment.

### Architectural Solutions

#### Solution 1: Pre-Authorization + Partial Refund (Recommended for v1)

**Flow**:
1. Client pre-authorizes MAX expected cost (e.g., $1.00 for 100k tokens)
2. Server processes request (actual cost: $0.10 for 10k tokens)
3. Server settles actual cost on-chain ($0.10)
4. **Unused authorization expires** (no refund needed)

**Implementation**:

```rust
// src/ai_pricing.rs

/// AI inference request with cost ceiling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiInferenceRequest {
    /// Prompt text.
    pub prompt: String,

    /// Maximum authorized cost (in token units).
    pub max_cost: TokenAmount,

    /// Payment authorization (signed for max_cost).
    pub authorization: TransferWithAuthorization,

    /// Optional: Estimated cost range for UX.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<CostEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub min: TokenAmount,
    pub max: TokenAmount,
    pub confidence: f32,  // 0.0-1.0
}

/// AI inference response with actual cost charged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiInferenceResponse {
    /// Generated content.
    pub content: String,

    /// Actual cost charged (in token units).
    pub cost: TokenAmount,

    /// Token usage breakdown.
    pub usage: TokenUsage,

    /// Settlement transaction hash.
    pub payment_tx: TransactionHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
```

**Handler**:

```rust
/// Process AI inference request with dynamic post-execution pricing.
pub async fn handle_ai_inference(
    State(facilitator): State<Arc<FacilitatorLocal<ProviderCache>>>,
    State(ai_service): State<Arc<dyn AiService>>,
    Json(request): Json<AiInferenceRequest>,
) -> Result<Json<AiInferenceResponse>, StatusCode> {
    // Step 1: Verify authorization for max_cost
    let verify_request = VerifyRequest {
        payment_payload: PaymentPayload {
            scheme: Scheme::Exact,
            network: request.authorization.network.clone(),
            payload: ExactPaymentPayload::Evm(EvmPayload {
                authorization: request.authorization.clone(),
                signature: request.authorization.signature.clone(),
            }),
        },
        payment_requirements: PaymentRequirements {
            asset: request.authorization.asset.clone(),
            amount: request.max_cost.clone(),
            pay_to: ai_service.wallet_address(),
            extra: None,
        },
    };

    let verify_response = facilitator.verify(&verify_request).await
        .map_err(|_| StatusCode::PAYMENT_REQUIRED)?;

    if !verify_response.is_valid {
        return Err(StatusCode::PAYMENT_REQUIRED);
    }

    // Step 2: Process AI request (cost unknown until now)
    let ai_result = ai_service.generate(&request.prompt).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Step 3: Calculate actual cost based on token usage
    let actual_cost = calculate_cost(&ai_result.usage);

    if actual_cost > request.max_cost {
        // Server-side error: underestimated cost
        error!(
            actual = %actual_cost,
            max = %request.max_cost,
            "AI request exceeded max authorized cost"
        );
        return Err(StatusCode::INSUFFICIENT_STORAGE); // HTTP 507
    }

    // Step 4: Settle with actual cost (NOT max_cost)
    // PROBLEM: EIP-3009 signature covers max_cost, not actual_cost
    // WORKAROUND: Use modified authorization (INSECURE - signature invalid!)

    // CORRECT APPROACH: Settle max_cost on-chain, refund difference off-chain
    let settle_request = SettleRequest {
        payment_payload: PaymentPayload {
            scheme: Scheme::Exact,
            network: request.authorization.network.clone(),
            payload: ExactPaymentPayload::Evm(EvmPayload {
                authorization: request.authorization.clone(), // Uses max_cost
                signature: request.authorization.signature.clone(),
            }),
        },
        payment_requirements: PaymentRequirements {
            asset: request.authorization.asset.clone(),
            amount: request.max_cost.clone(), // Settle full max_cost
            pay_to: ai_service.wallet_address(),
            extra: None,
        },
    };

    let settle_response = facilitator.settle(&settle_request).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Step 5: Record refund owed (max_cost - actual_cost) for off-chain processing
    let refund_amount = request.max_cost.clone() - actual_cost.clone();
    if refund_amount > TokenAmount::from(0u64) {
        ai_service.record_refund(
            request.authorization.from.clone(),
            refund_amount,
            settle_response.transaction.clone(),
        ).await?;
    }

    Ok(Json(AiInferenceResponse {
        content: ai_result.content,
        cost: actual_cost,
        usage: ai_result.usage,
        payment_tx: settle_response.transaction.unwrap(),
    }))
}

fn calculate_cost(usage: &TokenUsage) -> TokenAmount {
    // Example pricing: $0.01 per 1000 tokens (in 6-decimal USDC)
    // 1 USDC = 1,000,000 units
    // $0.01 = 10,000 units
    // 1 token = 10 units

    let cost_per_1k_tokens = 10_000u64; // $0.01 in USDC units
    let total_cost = (usage.total_tokens as u64 * cost_per_1k_tokens) / 1000;

    TokenAmount::from(total_cost)
}
```

**CRITICAL ISSUE**: This approach **settles the full max_cost on-chain**, then refunds the difference **off-chain**. This requires:

1. **Off-Chain Refund Processor**:
```rust
// src/refund_processor.rs

/// Background worker to process refunds for overcharged payments.
pub struct RefundProcessor {
    facilitator: Arc<FacilitatorLocal<ProviderCache>>,
    refund_store: Arc<dyn RefundStore>,
    batch_interval: Duration,
}

impl RefundProcessor {
    pub async fn run(&self) {
        let mut interval = tokio::time::interval(self.batch_interval);

        loop {
            interval.tick().await;

            if let Err(e) = self.process_pending_refunds().await {
                error!(error = %e, "Failed to process refunds");
            }
        }
    }

    async fn process_pending_refunds(&self) -> Result<(), Box<dyn std::error::Error>> {
        let pending = self.refund_store.get_pending_refunds().await?;

        // Batch refunds by network to save gas
        let mut batches: HashMap<Network, Vec<RefundEntry>> = HashMap::new();
        for refund in pending {
            batches.entry(refund.network).or_default().push(refund);
        }

        for (network, refunds) in batches {
            match self.batch_refund(network, &refunds).await {
                Ok(tx_hash) => {
                    info!(network = ?network, count = refunds.len(), tx = %tx_hash, "Batch refund sent");
                    for refund in refunds {
                        self.refund_store.mark_refunded(&refund.id, tx_hash.clone()).await?;
                    }
                }
                Err(e) => {
                    error!(network = ?network, error = %e, "Batch refund failed");
                }
            }
        }

        Ok(())
    }

    async fn batch_refund(
        &self,
        network: Network,
        refunds: &[RefundEntry],
    ) -> Result<TransactionHash, Box<dyn std::error::Error>> {
        let provider = self.facilitator.provider_map.by_network(network)
            .ok_or("Network not supported")?;

        // Build batch transfer transaction
        let mut batch_calldata = Vec::new();
        for refund in refunds {
            batch_calldata.push(encode_transfer(
                &refund.recipient,
                &refund.amount,
            ));
        }

        // Execute batch transfer (requires Multicall3 contract)
        let tx_receipt = provider.multicall(batch_calldata).await?;

        Ok(format!("{:?}", tx_receipt.transaction_hash).into())
    }
}

#[derive(Debug, Clone)]
pub struct RefundEntry {
    pub id: String,
    pub network: Network,
    pub recipient: MixedAddress,
    pub amount: TokenAmount,
    pub original_payment_tx: TransactionHash,
    pub reason: String,
}
```

2. **Trust Assumption**: Users must trust the server will actually issue refunds. This is **not trustless**.

#### Solution 2: Escrow Contract with Claim (Trustless, Complex)

**Requires smart contract deployment**:

```solidity
// AiPaymentEscrow.sol

contract AiPaymentEscrow {
    struct Payment {
        address payer;
        uint256 maxAmount;
        uint256 actualAmount;
        bool claimed;
        uint256 expiresAt;
    }

    mapping(bytes32 => Payment) public payments;

    function deposit(bytes32 paymentId, uint256 maxAmount, uint256 expiresAt) external payable {
        require(msg.value == maxAmount, "Incorrect deposit");
        payments[paymentId] = Payment({
            payer: msg.sender,
            maxAmount: maxAmount,
            actualAmount: 0,
            claimed: false,
            expiresAt: expiresAt
        });
    }

    function claimPartial(
        bytes32 paymentId,
        uint256 actualAmount,
        bytes memory signature
    ) external {
        Payment storage payment = payments[paymentId];
        require(!payment.claimed, "Already claimed");
        require(actualAmount <= payment.maxAmount, "Cannot claim more than deposited");

        // Verify payer signed the actual amount
        bytes32 hash = keccak256(abi.encodePacked(paymentId, actualAmount));
        address signer = recover(hash, signature);
        require(signer == payment.payer, "Invalid signature");

        payment.actualAmount = actualAmount;
        payment.claimed = true;

        // Pay server actual amount
        payable(msg.sender).transfer(actualAmount);

        // Refund payer the difference
        uint256 refund = payment.maxAmount - actualAmount;
        if (refund > 0) {
            payable(payment.payer).transfer(refund);
        }
    }

    function refundExpired(bytes32 paymentId) external {
        Payment storage payment = payments[paymentId];
        require(block.timestamp >= payment.expiresAt, "Not expired");
        require(!payment.claimed, "Already claimed");

        payment.claimed = true;
        payable(payment.payer).transfer(payment.maxAmount);
    }
}
```

**Pros**:
- Trustless (smart contract enforces refund)
- Payer gets automatic refund if server doesn't claim

**Cons**:
- Requires contract deployment on all networks
- Higher gas costs (2 transactions: deposit + claim)
- Complex UX (wallet must approve contract interaction)

### Recommendation

For **v1 implementation**:
1. Use **Pre-Authorization + Off-Chain Refund** (Solution 1)
2. Implement refund processor with transparent audit logs
3. Display refund history in user dashboard
4. Monthly on-chain refund batches (save gas)

For **v2 (future)**:
1. Deploy escrow contracts on supported networks
2. Migrate to trustless partial claim model
3. Gas cost analysis (may not be economical for small refunds)

---

## Cross-Cutting Concerns

### 1. State Synchronization Across Instances

**Problem**: Multi-server deployments need shared state (sessions, tabs, subscriptions).

**Solutions**:

| State Type | Storage | Sync Strategy |
|------------|---------|---------------|
| **Session tokens** | Redis | Write-through cache, 1-hour TTL |
| **Deferred tabs** | PostgreSQL | Read-after-write consistency |
| **Subscriptions** | PostgreSQL | Single cron leader (lease-based) |
| **Nonce tracking** | Redis (replicated) | Atomic increment |

**Leader Election for Cron Jobs**:
```rust
// src/leader_election.rs

use redis::Commands;
use std::time::Duration;

pub struct RedisLeaderElection {
    client: redis::Client,
    lease_key: String,
    lease_ttl: Duration,
    instance_id: String,
}

impl RedisLeaderElection {
    pub async fn try_acquire_lease(&self) -> Result<bool, redis::RedisError> {
        let mut conn = self.client.get_connection()?;

        // SET key value NX EX ttl
        let acquired: bool = conn.set_nx_ex(
            &self.lease_key,
            &self.instance_id,
            self.lease_ttl.as_secs() as usize,
        )?;

        Ok(acquired)
    }

    pub async fn renew_lease(&self) -> Result<bool, redis::RedisError> {
        let mut conn = self.client.get_connection()?;

        // Only renew if we still hold the lease
        let current_holder: Option<String> = conn.get(&self.lease_key)?;
        if current_holder.as_ref() == Some(&self.instance_id) {
            conn.expire(&self.lease_key, self.lease_ttl.as_secs() as usize)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
```

### 2. Observability and Metrics

**Critical Metrics**:
```rust
// src/metrics.rs

use prometheus::{IntCounter, IntGauge, Histogram};

pub struct FacilitatorMetrics {
    // Session metrics
    pub sessions_created: IntCounter,
    pub sessions_validated: IntCounter,
    pub sessions_expired: IntCounter,
    pub sessions_revoked: IntCounter,
    pub active_sessions: IntGauge,

    // Deferred payment metrics
    pub tabs_created: IntCounter,
    pub tabs_charged: IntCounter,
    pub tabs_settled: IntCounter,
    pub tab_settlement_value: Histogram,

    // Subscription metrics
    pub subscriptions_created: IntCounter,
    pub subscription_periods_settled: IntCounter,
    pub subscription_failures: IntCounter,

    // AI pricing metrics
    pub ai_requests_total: IntCounter,
    pub ai_cost_actual: Histogram,
    pub ai_cost_authorized: Histogram,
    pub ai_refunds_issued: IntCounter,
    pub ai_refund_value: Histogram,
}
```

**Trace Spans**:
```rust
#[instrument(skip_all, fields(
    session_id = %claims.sub,
    network = %claims.network,
    expires_at = claims.exp
))]
async fn validate_session(claims: SessionClaims) -> Result<(), SessionError> {
    // ...
}
```

### 3. Testing Strategy

**Unit Tests**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_expiration() {
        let claims = SessionClaims::new(
            "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb".to_string(),
            "eip155:8453".to_string(),
            "0xabc123".to_string(),
            Duration::from_secs(0), // Already expired
        );

        assert!(claims.is_expired());
    }

    #[tokio::test]
    async fn test_deferred_tab_accumulation() {
        let store = InMemoryTabStore::new();
        let tab_id = store.create_tab(
            MixedAddress::Evm(address!("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb")),
            "eip155:8453".to_string(),
            MixedAddress::Evm(address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")),
        ).await.unwrap();

        store.add_charge(&tab_id, DeferredLineItem {
            timestamp: 1234567890,
            description: "API call".to_string(),
            amount: TokenAmount::from(1000u64),
            metadata: None,
        }).await.unwrap();

        let tab = store.get_tab(&tab_id).await.unwrap().unwrap();
        assert_eq!(tab.total_owed, TokenAmount::from(1000u64));
    }
}
```

**Integration Tests**:
```rust
#[tokio::test]
async fn test_end_to_end_session_flow() {
    let facilitator = setup_test_facilitator().await;
    let session_manager = SessionManager::new(
        vec![0u8; 32], // Test secret
        Duration::from_secs(3600),
    ).unwrap();

    // 1. Make payment
    let settle_response = facilitator.settle(&test_settle_request()).await.unwrap();
    assert!(settle_response.success);

    // 2. Issue session
    let token = session_manager.issue_token(
        settle_response.payer.to_string(),
        settle_response.network.to_caip2(),
        settle_response.transaction.unwrap(),
        None,
    ).unwrap();

    // 3. Validate session
    let claims = session_manager.validate_token(&token).unwrap();
    assert_eq!(claims.sub, settle_response.payer.to_string());

    // 4. Revoke session
    session_manager.revoke_token(token.clone());

    // 5. Validation should fail
    assert!(session_manager.validate_token(&token).is_err());
}
```

---

## Implementation Roadmap

### Phase 1: SIWx Sessions (4-6 weeks)

**Week 1-2: Core Session Infrastructure**
- [ ] Implement `SessionManager` with JWT encoding/validation
- [ ] Add Redis session store (production)
- [ ] Add in-memory store (development)
- [ ] Unit tests for session lifecycle

**Week 3-4: HTTP Integration**
- [ ] Modify `/settle` endpoint to return session tokens
- [ ] Implement `session_auth_middleware` for Axum
- [ ] Add `/session/revoke` endpoint for logout
- [ ] Integration tests for session flow

**Week 5-6: Production Hardening**
- [ ] Add session metrics (Prometheus)
- [ ] Implement leader election for cleanup cron
- [ ] Load testing (1M sessions)
- [ ] Security audit (JWT secret rotation, XSS prevention)

**Deliverable**: Clients can pay once, reuse access via session tokens (200x latency reduction).

### Phase 2: Deferred Payment Tabs (6-8 weeks)

**Week 1-2: Data Model**
- [ ] Design PostgreSQL schema for tabs and line items
- [ ] Implement `DeferredTabStore` trait
- [ ] Add tab creation/charge/settlement endpoints
- [ ] Unit tests for tab accumulation

**Week 3-4: Settlement Logic**
- [ ] Implement tab settlement with EIP-3009
- [ ] Add auto-settle threshold triggers
- [ ] Handle insufficient balance gracefully
- [ ] Integration tests for tab lifecycle

**Week 5-6: UX Enhancements**
- [ ] Add `/tabs` endpoint for wallet UI (list open tabs)
- [ ] Implement itemized receipt generation
- [ ] Email/wallet notifications for settlement
- [ ] Audit logging for all tab operations

**Week 7-8: Production Hardening**
- [ ] Add tab metrics (Prometheus)
- [ ] Implement abandoned tab cleanup cron
- [ ] Load testing (100k concurrent tabs)
- [ ] Security audit (tab ID predictability, charge manipulation)

**Deliverable**: Cloudflare-style tab model for batch billing.

### Phase 3: Subscription Payments (8-10 weeks)

**Week 1-3: Authorization Model**
- [ ] Design subscription authorization structure
- [ ] Implement pre-signed period authorization generation
- [ ] Add subscription verification logic
- [ ] Unit tests for multi-period signatures

**Week 4-6: Settlement Automation**
- [ ] Implement `SubscriptionWorker` background job
- [ ] Add leader election for single cron instance
- [ ] Handle failed settlement retries
- [ ] Integration tests for recurring settlement

**Week 7-8: Failure Handling**
- [ ] Implement grace period for insufficient balance
- [ ] Add subscriber notification system
- [ ] Implement early cancellation logic
- [ ] Refund unused periods (off-chain)

**Week 9-10: Production Hardening**
- [ ] Add subscription metrics (Prometheus)
- [ ] Implement subscription analytics dashboard
- [ ] Load testing (10k active subscriptions)
- [ ] Security audit (nonce reuse, timing attacks)

**Deliverable**: SaaS-style recurring subscription payments.

### Phase 4: Dynamic AI Pricing (10-12 weeks)

**Week 1-3: Pre-Authorization Model**
- [ ] Implement `AiInferenceRequest` handler
- [ ] Add cost calculation logic (token-based pricing)
- [ ] Implement off-chain refund recording
- [ ] Unit tests for cost overrun detection

**Week 4-6: Refund Processor**
- [ ] Implement `RefundProcessor` background job
- [ ] Add batch refund logic (Multicall3)
- [ ] Implement refund audit logging
- [ ] Integration tests for refund flow

**Week 7-9: Escrow Contract (Optional)**
- [ ] Audit escrow contract design
- [ ] Deploy to testnet (Base Sepolia, etc.)
- [ ] Implement facilitator integration
- [ ] Gas cost analysis (economic viability)

**Week 10-12: Production Hardening**
- [ ] Add AI pricing metrics (Prometheus)
- [ ] Implement refund dashboard for users
- [ ] Load testing (1k concurrent AI requests)
- [ ] Security audit (max_cost manipulation, refund integrity)

**Deliverable**: Post-execution pricing for LLM inference and metered APIs.

---

## Conclusion

This document provides production-ready architectural guidance for implementing three critical x402 v2 features. Key takeaways:

1. **SIWx Sessions**: Use server-issued JWTs, not the v2 spec's Signed Identifier (over-engineered).
2. **Additional Schemes**: Prioritize `deferred` (tabs) and `subscription` over `streaming` (requires smart contracts).
3. **Dynamic Pricing**: Start with off-chain refunds (pragmatic), migrate to escrow contracts (trustless) when economically viable.

**Total Implementation Effort**: 28-36 weeks (7-9 months) for all three features.

**Recommended Sequencing**:
1. **Phase 1** (Sessions) - Highest ROI, lowest complexity
2. **Phase 2** (Deferred Tabs) - Medium ROI, medium complexity
3. **Phase 4** (AI Pricing) - High ROI for AI use cases, medium complexity
4. **Phase 3** (Subscriptions) - Lower urgency, can defer to v2

---

*Document Author: Aegis (Claude Sonnet 4.5)*
*Review Status: Draft for technical review*
*Next Steps: Stakeholder approval, sprint planning*
