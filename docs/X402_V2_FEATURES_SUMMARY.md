# x402 v2 Advanced Features: Executive Summary

**Full Document**: See `X402_V2_ADVANCED_FEATURES_ARCHITECTURE.md`
**Created**: 2025-12-23

---

## Quick Decisions Matrix

| Feature | Implement? | Complexity | ROI | Timeline | Dependencies |
|---------|-----------|------------|-----|----------|--------------|
| **SIWx Sessions** | YES (High Priority) | Low | Very High | 4-6 weeks | Redis |
| **Deferred Tabs** | YES (Medium Priority) | Medium | High | 6-8 weeks | PostgreSQL |
| **Subscriptions** | DEFER to v2 | High | Medium | 8-10 weeks | PostgreSQL + Cron |
| **AI Dynamic Pricing** | YES (High Priority) | Medium | Very High | 10-12 weeks | Refund system |
| **Streaming Payments** | NO (Requires contracts) | Very High | Low | 20+ weeks | Smart contracts |
| **UpTo Scheme** | NO (EIP-3009 incompatible) | Very High | Medium | N/A | Payment channels |

---

## Critical Architectural Insights

### 1. SIWx Sessions: IGNORE THE SPEC

**v2 Spec Proposes**: `SIGNED-IDENTIFIER` header (client signs every request)

**Why This is Wrong**:
- Smart accounts (EIP-1271) require RPC call per request (~200ms overhead)
- Short expiry times mean constant re-signing
- Defeats the purpose of sessions

**Our Approach**: Server-issued JWTs (like OAuth2)
- Validation: HMAC (~1μs) vs RPC call (~200ms) = **200x faster**
- Works with smart accounts (no RPC needed)
- Battle-tested (OAuth2, OIDC standards)
- Revocation: Server-controlled (better UX)

**Code Location**: `src/session.rs`, `src/session_store.rs`

### 2. Dynamic Pricing: The EIP-3009 Problem

**Challenge**: EIP-3009 signatures cover EXACT amount. You cannot:
- Sign for max_amount ($1.00)
- Settle for actual_amount ($0.10)
- Signature becomes invalid

**v1 Solution (Pragmatic)**:
1. Settle max_amount on-chain
2. Refund difference off-chain (batch monthly)
3. Trust assumption (not ideal but workable)

**v2 Solution (Trustless)**:
1. Deploy escrow contracts
2. Client deposits max_amount
3. Server claims actual_amount
4. Contract refunds difference automatically
5. **Cons**: Higher gas, complex UX, contract audits

**Recommendation**: Start with v1 (off-chain refunds), migrate to v2 when volume justifies gas costs.

### 3. Payment Schemes: What Actually Works

| Scheme | Status | Reason |
|--------|--------|--------|
| `exact` | PRODUCTION | EIP-3009 native support |
| `fhe-transfer` | PRODUCTION | Proxied to Zama Lambda |
| `deferred` | IMPLEMENT | PostgreSQL-backed tabs |
| `subscription` | IMPLEMENT | Pre-signed period authorizations |
| `upto` | BLOCKED | EIP-3009 incompatible without escrow |
| `streaming` | BLOCKED | Requires payment channel contracts |

**Why `upto` Doesn't Work**:
```rust
// PROBLEM: This signature is invalid
let auth = TransferWithAuthorization {
    value: max_amount,  // Signed for $1.00
    // ... other fields
};

// Server cannot do this:
auth.value = actual_amount;  // Change to $0.10 - SIGNATURE BREAKS

// EIP-3009 verifies signature against EXACT value field
```

**Solution**: Escrow contracts (future work).

### 4. State Management: Storage Backend Selection

| Data Type | Storage | Why |
|-----------|---------|-----|
| **Sessions** | Redis | Fast (1ms), TTL auto-cleanup, distributed |
| **Tabs** | PostgreSQL | ACID guarantees, complex queries, audit trails |
| **Subscriptions** | PostgreSQL | Cron job queries, transactional safety |
| **Nonce Tracking** | Redis | Atomic increments, replay protection |
| **Refunds** | PostgreSQL | Financial audit requirements, CANNOT lose data |

**Anti-Pattern**: Using in-memory storage (Arc<RwLock<HashMap>>) for production
- Lost on restart
- No cross-instance sharing
- No audit trail

---

## Type System Additions Required

### Session Types (`src/session.rs`)

```rust
pub struct SessionClaims {
    pub sub: String,           // Wallet address
    pub iat: u64,              // Issued at
    pub exp: u64,              // Expiration
    pub network: String,       // CAIP-2 network
    pub payment_tx: String,    // Settlement tx hash
    pub scopes: Option<Vec<String>>,
    pub amount_paid: Option<String>,
}

pub struct SessionManager {
    jwt_secret: Vec<u8>,       // MUST be 256+ bits
    default_ttl: Duration,
    revoked_sessions: Arc<RwLock<HashSet<String>>>,
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn store_session(&self, token_hash: String, claims: SessionClaims) -> Result<(), StoreError>;
    async fn retrieve_session(&self, token_hash: &str) -> Result<Option<SessionClaims>, StoreError>;
    async fn revoke_session(&self, token_hash: &str) -> Result<(), StoreError>;
    async fn cleanup_expired(&self) -> Result<usize, StoreError>;
}
```

### Deferred Payment Types (`src/types.rs`)

```rust
pub struct DeferredPaymentTab {
    pub tab_id: String,
    pub payer: MixedAddress,
    pub network: String,
    pub asset: MixedAddress,
    pub total_owed: TokenAmount,
    pub line_items: Vec<DeferredLineItem>,
    pub created_at: u64,
    pub auto_settle_threshold: Option<TokenAmount>,
}

pub struct DeferredLineItem {
    pub timestamp: u64,
    pub description: String,
    pub amount: TokenAmount,
    pub metadata: Option<serde_json::Value>,
}

#[async_trait]
pub trait DeferredTabStore: Send + Sync {
    async fn create_tab(&self, payer: MixedAddress, network: String, asset: MixedAddress) -> Result<String, StoreError>;
    async fn add_charge(&self, tab_id: &str, item: DeferredLineItem) -> Result<(), StoreError>;
    async fn get_tab(&self, tab_id: &str) -> Result<Option<DeferredPaymentTab>, StoreError>;
    async fn close_tab(&self, tab_id: &str) -> Result<DeferredPaymentTab, StoreError>;
}
```

### AI Pricing Types (`src/ai_pricing.rs`)

```rust
pub struct AiInferenceRequest {
    pub prompt: String,
    pub max_cost: TokenAmount,
    pub authorization: TransferWithAuthorization,
    pub estimated_cost: Option<CostEstimate>,
}

pub struct AiInferenceResponse {
    pub content: String,
    pub cost: TokenAmount,            // Actual cost charged
    pub usage: TokenUsage,            // Token breakdown
    pub payment_tx: TransactionHash,
}

pub struct RefundEntry {
    pub id: String,
    pub network: Network,
    pub recipient: MixedAddress,
    pub amount: TokenAmount,
    pub original_payment_tx: TransactionHash,
    pub reason: String,
}
```

### Subscription Types (`src/subscription.rs`)

```rust
pub struct SubscriptionAuthorization {
    pub subscriber: MixedAddress,
    pub payee: MixedAddress,
    pub amount_per_period: TokenAmount,
    pub period_seconds: u64,          // E.g., 2592000 = 30 days
    pub total_periods: u32,           // E.g., 12 for annual
    pub starts_at: u64,
    pub period_authorizations: Vec<TransferWithAuthorization>,  // Pre-signed
}

#[async_trait]
pub trait SubscriptionStore: Send + Sync {
    async fn create_subscription(&self, sub: SubscriptionAuthorization) -> Result<String, StoreError>;
    async fn get_due_subscriptions(&self, now: u64) -> Result<Vec<(String, usize)>, StoreError>;
    async fn mark_period_settled(&self, sub_id: &str, period: usize) -> Result<(), StoreError>;
}
```

---

## Handler Modifications Required

### 1. `/settle` Endpoint - Add Session Token

```rust
// src/handlers.rs

pub async fn post_settle_with_session<A>(
    State(facilitator): State<A>,
    State(session_manager): State<Arc<SessionManager>>,  // NEW
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator,
{
    // ... existing settlement logic ...

    match facilitator.settle(&body).await {
        Ok(settle_response) if settle_response.success => {
            // NEW: Issue session token
            let session_token = session_manager.issue_token(
                settle_response.payer.to_string(),
                settle_response.network.to_caip2(),
                settle_response.transaction.clone().unwrap(),
                None,
            ).ok();

            // Return extended response
            Json(json!({
                "success": true,
                "transaction": settle_response.transaction,
                "payer": settle_response.payer,
                "network": settle_response.network,
                "sessionToken": session_token,  // NEW
            }))
        }
        // ... error handling ...
    }
}
```

### 2. Session Authentication Middleware

```rust
// src/handlers.rs

pub async fn session_auth_middleware(
    State(session_manager): State<Arc<SessionManager>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = req.headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = session_manager.validate_token(auth_header)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
```

### 3. New Endpoints for Tabs

```rust
// src/handlers.rs

/// POST /tabs - Create new deferred payment tab
pub async fn create_tab(
    State(tab_store): State<Arc<dyn DeferredTabStore>>,
    Json(request): Json<CreateTabRequest>,
) -> impl IntoResponse {
    let tab_id = tab_store.create_tab(
        request.payer,
        request.network,
        request.asset,
    ).await?;

    Json(json!({ "tabId": tab_id }))
}

/// POST /tabs/{tab_id}/charge - Add charge to existing tab
pub async fn add_tab_charge(
    State(tab_store): State<Arc<dyn DeferredTabStore>>,
    Path(tab_id): Path<String>,
    Json(item): Json<DeferredLineItem>,
) -> impl IntoResponse {
    tab_store.add_charge(&tab_id, item).await?;
    StatusCode::OK
}

/// POST /tabs/{tab_id}/settle - Close tab and settle total
pub async fn settle_tab(
    State(facilitator): State<Arc<FacilitatorLocal<ProviderCache>>>,
    State(tab_store): State<Arc<dyn DeferredTabStore>>,
    Path(tab_id): Path<String>,
    Json(settle_request): Json<DeferredSettleRequest>,
) -> impl IntoResponse {
    let tab = tab_store.get_tab(&tab_id).await?.ok_or(StatusCode::NOT_FOUND)?;

    // Verify authorization signature covers total_owed
    // ... verification logic ...

    // Settle payment
    let settle_response = facilitator.settle(&build_settle_request(&tab, &settle_request)).await?;

    // Close tab
    tab_store.close_tab(&tab_id).await?;

    Json(settle_response)
}
```

---

## Security Checklist

### Sessions
- [ ] JWT secret is 256+ bits (32+ bytes) cryptographically random
- [ ] Secret stored in AWS Secrets Manager (NOT env vars)
- [ ] Different secrets for mainnet vs testnet
- [ ] Session TTL <= 1 hour for high-value content
- [ ] Revocation list cleaned up periodically (prevent memory leak)
- [ ] Session claims include network (prevent cross-chain reuse)

### Deferred Tabs
- [ ] Tab IDs are UUID v4 (unpredictable)
- [ ] Verify payer matches tab owner before adding charges
- [ ] Rate-limit tab creation (prevent DoS)
- [ ] All charges logged with timestamp (audit trail)
- [ ] Cannot modify historical charges (append-only)
- [ ] Auto-close abandoned tabs after 30 days

### AI Pricing
- [ ] Validate actual_cost <= max_cost (prevent overcharge)
- [ ] Refunds recorded before settlement (atomic transaction)
- [ ] Batch refunds monthly (save gas)
- [ ] Transparent cost breakdown in response
- [ ] Alert on refund failures (financial integrity)

### Subscriptions
- [ ] Each period authorization has unique nonce
- [ ] Nonces are sequential (prevent period skipping)
- [ ] Track used nonces in database (replay protection)
- [ ] Grace period before cancellation (3 days retry)
- [ ] Notify subscriber before auto-cancel
- [ ] Leader election for single cron instance

---

## Performance Impact Analysis

| Feature | Latency Impact | Memory Impact | Database Load | Gas Cost Impact |
|---------|----------------|---------------|---------------|-----------------|
| **Sessions** | -200ms (200x faster) | +10MB per 10k sessions | Read-heavy (Redis) | None (off-chain) |
| **Deferred Tabs** | +5ms (DB write) | Minimal | Write-heavy (PostgreSQL) | Batch savings (~50%) |
| **Subscriptions** | Async (cron) | Minimal | Read-medium (cron queries) | Standard per period |
| **AI Pricing** | +100ms (refund recording) | Minimal | Write-medium (refund log) | +gas for refund tx |

**Key Optimization**: Sessions reduce repeated payment verification from 200-500ms to <1ms (HMAC validation).

---

## Dependencies and Infrastructure

### New Dependencies (`Cargo.toml`)

```toml
[dependencies]
# Sessions
jsonwebtoken = "9.2"           # JWT encoding/decoding
redis = { version = "0.24", features = ["tokio-comp", "connection-manager"] }

# Deferred tabs
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "postgres", "uuid"] }
uuid = { version = "1.6", features = ["v4"] }

# Background workers
tokio = { version = "1.35", features = ["full"] }
tokio-cron-scheduler = "0.10"

# Metrics
prometheus = "0.13"
```

### Infrastructure Requirements

**Redis** (Sessions + Nonce Tracking):
- AWS ElastiCache (Redis 7.0+)
- Instance: cache.t3.micro ($13/month) for dev, cache.m6g.large ($88/month) for prod
- Replication: 1 primary + 1 replica (HA)

**PostgreSQL** (Tabs + Subscriptions):
- AWS RDS PostgreSQL 15
- Instance: db.t3.micro ($12/month) for dev, db.m6g.large ($145/month) for prod
- Storage: 20GB gp3 ($2.40/month)
- Backups: Automated daily snapshots

**Background Workers**:
- Subscription settlement cron: Every 1 hour
- Refund processor: Every 24 hours (batch)
- Session cleanup: Every 1 hour
- Tab cleanup: Every 24 hours

---

## Migration Path

### Phase 1: Sessions (Weeks 1-6)

**Week 1-2**: Core infrastructure
```bash
# Add dependencies
cargo add jsonwebtoken redis uuid

# Create new modules
touch src/session.rs
touch src/session_store.rs

# Update lib.rs
echo "pub mod session;" >> src/lib.rs
echo "pub mod session_store;" >> src/lib.rs
```

**Week 3-4**: HTTP integration
```rust
// main.rs modifications

let session_manager = Arc::new(SessionManager::new(
    std::env::var("JWT_SECRET")?.as_bytes().to_vec(),
    Duration::from_secs(3600),
)?);

let app = Router::new()
    .route("/settle", post(post_settle_with_session))
    .layer(Extension(session_manager.clone()));
```

**Week 5-6**: Production deployment
1. Provision AWS ElastiCache Redis cluster
2. Update ECS task definition with Redis endpoint
3. Deploy to staging, run integration tests
4. Gradual rollout to production (10% -> 50% -> 100%)

### Phase 2: Deferred Tabs (Weeks 7-14)

**Week 7-8**: Database schema
```sql
CREATE TABLE deferred_tabs (
    tab_id TEXT PRIMARY KEY,
    payer TEXT NOT NULL,
    network TEXT NOT NULL,
    asset TEXT NOT NULL,
    total_owed NUMERIC NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open'
);

CREATE TABLE deferred_line_items (
    id SERIAL PRIMARY KEY,
    tab_id TEXT NOT NULL REFERENCES deferred_tabs(tab_id),
    timestamp BIGINT NOT NULL,
    description TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    metadata JSONB
);
```

**Week 9-10**: API endpoints
- [ ] POST /tabs
- [ ] POST /tabs/{id}/charge
- [ ] GET /tabs/{id}
- [ ] POST /tabs/{id}/settle

**Week 11-12**: Settlement logic
- [ ] Implement tab settlement handler
- [ ] Add auto-settle threshold triggers
- [ ] Handle insufficient balance cases

**Week 13-14**: Production deployment
1. Provision AWS RDS PostgreSQL
2. Run migrations on staging
3. Integration tests
4. Production rollout

### Phase 3: AI Dynamic Pricing (Weeks 15-26)

Similar phased approach - see full document.

---

## Questions for Stakeholders

1. **Sessions**: What is acceptable session TTL for your use case?
   - High security: 15 minutes
   - Normal: 1 hour
   - Low risk content: 24 hours

2. **Deferred Tabs**: What is auto-settle threshold?
   - Option A: Fixed amount (e.g., $10 accumulated)
   - Option B: Time-based (e.g., monthly)
   - Option C: Both (whichever comes first)

3. **AI Pricing**: Refund strategy?
   - Option A: Off-chain refunds (trust required, simple)
   - Option B: Escrow contracts (trustless, complex)
   - Option C: No refunds (charge max, simpler but poor UX)

4. **Subscriptions**: Retry policy for failed payments?
   - Option A: 3-day grace period, then cancel
   - Option B: Suspend service, allow manual retry
   - Option C: Immediate cancellation

---

## Next Steps

1. **Technical Review**: Senior engineers review this architecture
2. **Stakeholder Approval**: Product team prioritizes features
3. **Sprint Planning**: Break into 2-week sprints
4. **Prototype**: Build Phase 1 (Sessions) proof-of-concept
5. **Integration Tests**: Ensure backward compatibility with v1 clients
6. **Production Deployment**: Gradual rollout with feature flags

---

*Summary Document*
*Full Technical Details: X402_V2_ADVANCED_FEATURES_ARCHITECTURE.md*
*Author: Aegis (Claude Sonnet 4.5)*
