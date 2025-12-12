# x402 v2 Master Action Plan
## "The Best x402 v2 Facilitator in the World"

**Document Version:** 2.0 (Zero-Downtime Edition)
**Date:** 2025-12-11
**Last Updated:** 2025-12-11 (Critical safety review by Gemini)
**Synthesized From:**
- Gemini Deep Research (7 areas)
- Gemini Critical Safety Review (deployment risks)
- Aegis Rust Architect Analysis
- Terraform AWS Architect Analysis
- Task Decomposition Expert Framework

---

## CRITICAL SAFETY NOTICE

```
============================================================================
                    PRODUCTION SYSTEM - MILLIONS AT STAKE
============================================================================

This facilitator processes 1000s of transactions. A deployment failure
could cause MILLIONS OF DOLLARS in losses.

MANDATORY REQUIREMENTS:
1. BLUE-GREEN deployment (NOT rolling update)
2. Nonce safety (fetch fresh before EVERY transaction)
3. Graceful shutdown (handle SIGTERM, complete in-flight txs)
4. Feature flags (disable v2 without redeployment)
5. Automatic rollback triggers
6. Shadow testing before live traffic

DO NOT SKIP ANY SAFETY TASK - THEY ARE NOT OPTIONAL
============================================================================
```

---

## Executive Summary

This document presents the comprehensive, task-decomposed action plan for migrating the Ultravioleta DAO x402-rs facilitator to protocol v2. The plan was created through collaborative multi-agent analysis and critically reviewed for zero-downtime safety.

**Key Metrics:**
- **Total Tasks:** 89 tasks across 7 phases (including new Phase 0: Safety)
- **Estimated Effort:** 100-140 hours of development
- **Infrastructure Cost Impact:** +$50/month (blue-green + CloudWatch)
- **Target Version:** v2.0.0
- **Migration Period:** 6 months dual-support, then deprecate v1

**Safety-First Approach:**
1. Blue-green deployment (instant rollback capability)
2. Fresh nonce fetch before every transaction (no caching during transition)
3. Graceful shutdown with in-flight transaction completion
4. Feature flags for surgical v2 disable
5. Shadow testing with production traffic
6. Automatic rollback on error spike

---

## Phase Overview

| Phase | Name | Duration | Tasks | Priority |
|-------|------|----------|-------|----------|
| **0** | **Safety Infrastructure** | **Week 0-1** | **22** | **MANDATORY** |
| 1 | Foundation - CAIP-2 Types | Week 1-2 | 15 | CRITICAL |
| 2 | v2 Core Types | Week 2-3 | 14 | CRITICAL |
| 3 | Handler Updates | Week 3-4 | 12 | HIGH |
| 4 | Testing & Quality | Week 4-5 | 10 | HIGH |
| 5 | Deployment | Week 5-6 | 9 | CRITICAL |
| 6 | Migration & Deprecation | Month 2-6 | 7 | MEDIUM |

---

## Phase 0: Safety Infrastructure (MANDATORY)
**Duration:** Week 0-1 (BEFORE any v2 code)
**Agent:** aegis-rust-architect + terraform-aws-architect
**Priority:** MANDATORY - DO NOT SKIP

### Overview
Implement all safety mechanisms BEFORE writing any v2 code. This phase ensures we can deploy, monitor, and rollback safely.

### 0.1 Blue-Green Infrastructure
**Why:** Rolling deployments are NOT SAFE for this service due to EVM nonce caching.

- [ ] **Task 0.1.1:** Create second ECS service for blue-green
  ```hcl
  # terraform/environments/production/blue-green.tf
  resource "aws_ecs_service" "facilitator_green" {
    name            = "facilitator-green"
    cluster         = aws_ecs_cluster.facilitator.id
    task_definition = aws_ecs_task_definition.facilitator_v2.arn
    desired_count   = 0  # Start with 0, scale up for deployment
    # ... same config as blue
  }
  ```
  - **File:** `terraform/environments/production/blue-green.tf` (new)
  - **Cost:** +$45/month when green is active

- [ ] **Task 0.1.2:** Create ALB target group for green service
  ```hcl
  resource "aws_lb_target_group" "facilitator_green" {
    name     = "facilitator-green-tg"
    port     = 8080
    protocol = "HTTP"
    vpc_id   = aws_vpc.facilitator.id
    # Same health check as blue
  }
  ```

- [ ] **Task 0.1.3:** Create weighted routing rule for gradual traffic shift
  ```hcl
  resource "aws_lb_listener_rule" "weighted" {
    listener_arn = aws_lb_listener.https.arn
    action {
      type = "forward"
      forward {
        target_group {
          arn    = aws_lb_target_group.facilitator_blue.arn
          weight = var.blue_weight  # Start at 100
        }
        target_group {
          arn    = aws_lb_target_group.facilitator_green.arn
          weight = var.green_weight  # Start at 0
        }
      }
    }
  }
  ```

- [ ] **Task 0.1.4:** Create traffic shift script
  ```bash
  # scripts/shift-traffic.sh
  # Usage: ./shift-traffic.sh 0    # 100% blue
  # Usage: ./shift-traffic.sh 10   # 90% blue, 10% green
  # Usage: ./shift-traffic.sh 100  # 100% green
  ```

- [ ] **Task 0.1.5:** Create instant rollback script
  ```bash
  # scripts/instant-rollback.sh
  # Immediately shifts 100% traffic to blue (v1)
  # Takes < 5 seconds
  ```

### 0.2 Nonce Safety (CRITICAL)
**Why:** EVM `PendingNonceManager` caches nonces in memory. During deployment with multiple instances, nonce collisions cause transaction failures.

- [ ] **Task 0.2.1:** Add `NONCE_FETCH_MODE` environment variable
  ```rust
  // src/chain/evm.rs
  pub enum NonceFetchMode {
      Cached,      // Default (current behavior)
      AlwaysFresh, // Fetch from chain before EVERY tx
  }
  ```
  - **File:** `src/chain/evm.rs`

- [ ] **Task 0.2.2:** Implement `always_fresh` nonce fetching
  ```rust
  async fn get_nonce(&self, address: Address) -> Result<u64> {
      match self.nonce_mode {
          NonceFetchMode::AlwaysFresh => {
              // Always query blockchain, ignore cache
              self.provider.get_transaction_count(address).pending().await
          }
          NonceFetchMode::Cached => {
              // Existing PendingNonceManager behavior
              self.nonce_manager.get_nonce(address).await
          }
      }
  }
  ```

- [ ] **Task 0.2.3:** Set `NONCE_FETCH_MODE=always_fresh` during v2 transition
  - **File:** `terraform/environments/production/main.tf`
  - **Note:** Can revert to `cached` after migration complete and single instance

- [ ] **Task 0.2.4:** Add nonce mismatch detection and logging
  ```rust
  // Log warning if cached nonce differs from on-chain
  let cached = self.nonce_manager.peek(address);
  let fresh = self.provider.get_transaction_count(address).pending().await?;
  if cached != fresh {
      tracing::warn!(
          cached_nonce = cached,
          fresh_nonce = fresh,
          address = %address,
          "Nonce mismatch detected - using fresh nonce"
      );
  }
  ```

### 0.3 Graceful Shutdown (CRITICAL)
**Why:** ECS sends SIGTERM during deployment. In-flight transactions must complete.

- [ ] **Task 0.3.1:** Verify `SigDown` utility handles SIGTERM correctly
  - **File:** `src/sig_down.rs`
  - **Status:** Already implemented, need to verify behavior

- [ ] **Task 0.3.2:** Increase ECS deregistration delay to 120 seconds
  ```hcl
  # Allow 2 minutes for in-flight settlements to complete
  deregistration_delay = 120
  ```
  - **File:** `terraform/environments/production/main.tf`

- [ ] **Task 0.3.3:** Add shutdown logging
  ```rust
  tracing::info!("Received shutdown signal, completing in-flight transactions...");
  // ... wait for completion ...
  tracing::info!(
      completed_count = count,
      "Graceful shutdown complete"
  );
  ```

- [ ] **Task 0.3.4:** Add in-flight transaction counter
  ```rust
  static IN_FLIGHT_SETTLEMENTS: AtomicU32 = AtomicU32::new(0);

  // Increment at start of settle()
  IN_FLIGHT_SETTLEMENTS.fetch_add(1, Ordering::SeqCst);
  // Decrement at end (in Drop guard)
  ```

- [ ] **Task 0.3.5:** Block shutdown until in-flight count is zero
  ```rust
  async fn wait_for_in_flight() {
      while IN_FLIGHT_SETTLEMENTS.load(Ordering::SeqCst) > 0 {
          tokio::time::sleep(Duration::from_millis(100)).await;
      }
  }
  ```

### 0.4 Feature Flags
**Why:** Need to disable v2 surgically without full redeployment.

- [ ] **Task 0.4.1:** Create feature flag module
  ```rust
  // src/feature_flags.rs
  pub struct FeatureFlags {
      pub v2_enabled: bool,
      pub v2_verify_enabled: bool,
      pub v2_settle_enabled: bool,
      pub v2_networks: HashSet<Network>,  // Enable per-network
  }
  ```
  - **File:** `src/feature_flags.rs` (new)

- [ ] **Task 0.4.2:** Load feature flags from environment
  ```rust
  // V2_ENABLED=true
  // V2_SETTLE_ENABLED=false  (can disable just settlement)
  // V2_NETWORKS=base,ethereum,polygon  (enable specific networks)
  ```

- [ ] **Task 0.4.3:** Add feature flag checks to handlers
  ```rust
  if !feature_flags.v2_enabled {
      // Reject v2 requests with clear error
      return Err(Error::V2Disabled);
  }
  ```

- [ ] **Task 0.4.4:** Add `/flags` endpoint for runtime inspection
  ```rust
  // GET /flags -> { "v2_enabled": true, "v2_settle_enabled": false, ... }
  ```

### 0.5 Deep Health Checks
**Why:** `/health` only checks if process is running, not if v2 logic works.

- [ ] **Task 0.5.1:** Create `/health/deep` endpoint
  ```rust
  // Validates:
  // 1. CAIP-2 parsing works
  // 2. v2 type serialization works
  // 3. RPC connections healthy
  // 4. Feature flags loaded correctly
  ```
  - **File:** `src/handlers.rs`

- [ ] **Task 0.5.2:** Create `/health/v2` endpoint
  ```rust
  // Specifically validates v2 functionality
  // Returns 503 if v2 has issues
  ```

- [ ] **Task 0.5.3:** Configure ALB to use `/health/deep` for green target group
  ```hcl
  health_check {
    path                = "/health/deep"
    healthy_threshold   = 3
    unhealthy_threshold = 2
    timeout             = 10
    interval            = 15
  }
  ```

### 0.6 Automatic Rollback
**Why:** Manual rollback takes minutes. Automatic rollback takes seconds.

- [ ] **Task 0.6.1:** Create CloudWatch alarm for settlement failure spike
  ```hcl
  resource "aws_cloudwatch_metric_alarm" "settlement_failure_spike" {
    alarm_name          = "facilitator-settlement-failure-spike"
    comparison_operator = "GreaterThanThreshold"
    evaluation_periods  = 2
    metric_name         = "SettlementFailures"
    namespace           = "Facilitator/Production"
    period              = 60
    statistic           = "Sum"
    threshold           = 5  # 5 failures in 2 minutes
    alarm_actions       = [aws_lambda_function.auto_rollback.arn]
  }
  ```

- [ ] **Task 0.6.2:** Create Lambda for automatic rollback
  ```python
  # lambda/auto_rollback.py
  def handler(event, context):
      # Shift 100% traffic to blue (v1)
      # Send alert to PagerDuty/Slack
      # Log rollback reason
  ```

- [ ] **Task 0.6.3:** Add manual rollback override
  ```bash
  # Disable auto-rollback for planned maintenance
  aws lambda update-function-configuration \
    --function-name auto-rollback \
    --environment "Variables={ENABLED=false}"
  ```

### 0.7 Shadow Testing
**Why:** Test v2 with production traffic without risk.

- [ ] **Task 0.7.1:** Add shadow mode to handlers
  ```rust
  // In shadow mode:
  // 1. Process v1 request normally (return result)
  // 2. Also process as v2 (log result, don't return)
  // 3. Compare results, log discrepancies
  ```

- [ ] **Task 0.7.2:** Add `SHADOW_V2_ENABLED` environment variable
- [ ] **Task 0.7.3:** Add shadow comparison logging
  ```rust
  if v1_result != v2_result {
      tracing::error!(
          v1_result = ?v1_result,
          v2_result = ?v2_result,
          request_id = %request_id,
          "Shadow test: v1/v2 result mismatch!"
      );
  }
  ```

### Phase 0 Deliverables
- Blue-green infrastructure with instant rollback
- Nonce safety for multi-instance deployments
- Graceful shutdown with in-flight completion
- Feature flags for surgical v2 disable
- Deep health checks for v2 validation
- Automatic rollback on failure spike
- Shadow testing capability

### Phase 0 Verification Checklist
- [ ] Blue-green terraform applies without error
- [ ] Traffic shift script works (test with 0%, 50%, 100%)
- [ ] Instant rollback completes in < 5 seconds
- [ ] `NONCE_FETCH_MODE=always_fresh` works correctly
- [ ] Graceful shutdown waits for in-flight transactions
- [ ] Feature flags can disable v2 at runtime
- [ ] `/health/deep` validates v2 functionality
- [ ] Auto-rollback Lambda triggers on alarm
- [ ] Shadow mode logs v1/v2 comparisons

---

## Phase 1: Foundation - CAIP-2 Types
**Duration:** Week 1-2
**Agent:** aegis-rust-architect
**Files:** `src/caip2.rs` (new), `src/network.rs`
**Prerequisite:** Phase 0 complete

### Overview
Implement CAIP-2 network identifier support with zero-cost abstractions. This is the foundation for all v2 functionality.

### Tasks

#### 1.1 Create CAIP-2 Module
- [ ] **Task 1.1.1:** Create `src/caip2.rs` with `Namespace` enum
  - Variants: `Eip155`, `Solana`, `Near`, `Stellar`, `Fogo`
  - Implement `Display`, `FromStr`, `Serialize`, `Deserialize`
  - **File:** `src/caip2.rs:1-50`
  - **Reference:** `docs/X402_V2_TYPE_SYSTEM_DESIGN.md` Section 1.1

- [ ] **Task 1.1.2:** Implement `Caip2NetworkId` struct
  - Fields: `namespace: Namespace`, `reference: String`
  - Constructor with validation per namespace
  - **File:** `src/caip2.rs:51-150`

- [ ] **Task 1.1.3:** Implement CAIP-2 parsing and formatting
  - `parse(s: &str) -> Result<Self, Caip2ParseError>`
  - `to_string() -> String`
  - **File:** `src/caip2.rs:151-200`

- [ ] **Task 1.1.4:** Add custom Serde serialization
  - Serialize to canonical string format
  - Deserialize with validation
  - **File:** `src/caip2.rs:201-250`

#### 1.2 CAIP-2 Error Handling
- [ ] **Task 1.2.1:** Create `Caip2ParseError` enum
  - Variants for all failure modes
  - Implement `thiserror::Error`
  - **File:** `src/caip2.rs:251-280`

#### 1.3 Network <-> CAIP-2 Conversion
- [ ] **Task 1.3.1:** Add `to_caip2()` method to `Network` enum
  - Map each network to CAIP-2 identifier
  - Handle all 20+ networks
  - **File:** `src/network.rs` (new impl block)

- [ ] **Task 1.3.2:** Add `from_caip2()` method to `Network` enum
  - Parse CAIP-2 and return matching network
  - Return error for unsupported chains
  - **File:** `src/network.rs` (new impl block)

- [ ] **Task 1.3.3:** Define CAIP-2 constants for all networks
  ```rust
  // EVM (eip155:{chainId})
  BASE_MAINNET = "eip155:8453"
  BASE_SEPOLIA = "eip155:84532"
  ETHEREUM = "eip155:1"
  // ... all 23 EVM networks

  // Non-EVM (custom namespaces)
  SOLANA_MAINNET = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
  NEAR_MAINNET = "near:mainnet"
  STELLAR_PUBNET = "stellar:pubnet"
  FOGO_MAINNET = "fogo:mainnet"
  ```
  - **File:** `src/network.rs` (constants section)

#### 1.4 Genesis Hash Handling
- [ ] **Task 1.4.1:** Add Solana genesis hash constants
  - Mainnet: `5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp`
  - Devnet: `EtWTRABZaYq6iMfeYKouRu166VU2xqa1`
  - **File:** `src/network.rs`

- [ ] **Task 1.4.2:** Add genesis hash validation for Solana
  - Base58 format validation
  - Length check (32 bytes)
  - **File:** `src/caip2.rs`

#### 1.5 Module Integration
- [ ] **Task 1.5.1:** Export `caip2` module from `src/lib.rs`
- [ ] **Task 1.5.2:** Add `mod caip2;` to `src/main.rs`
- [ ] **Task 1.5.3:** Update `Cargo.toml` if new dependencies needed

#### 1.6 Unit Tests
- [ ] **Task 1.6.1:** Test CAIP-2 parsing (valid cases)
- [ ] **Task 1.6.2:** Test CAIP-2 parsing (invalid cases)
- [ ] **Task 1.6.3:** Test Network <-> CAIP-2 round-trip for all networks

### Deliverables
- `src/caip2.rs` - New 350-line module
- Updated `src/network.rs` with conversion methods
- 100% test coverage for CAIP-2 types

---

## Phase 2: v2 Core Types
**Duration:** Week 2-3
**Agent:** aegis-rust-architect
**Files:** `src/types_v2.rs` (new), `src/types.rs`

### Overview
Implement v2 protocol types while maintaining backward compatibility with v1.

### Tasks

#### 2.1 ResourceInfo Type
- [ ] **Task 2.1.1:** Create `ResourceInfo` struct
  ```rust
  pub struct ResourceInfo {
      pub url: Url,
      pub description: String,
      pub mime_type: Option<String>,
  }
  ```
  - **File:** `src/types_v2.rs:1-30`

#### 2.2 PaymentRequirements v2
- [ ] **Task 2.2.1:** Create `PaymentRequirementsV2` struct
  ```rust
  pub struct PaymentRequirementsV2 {
      pub scheme: Scheme,
      pub network: Caip2NetworkId,  // Not Network enum
      pub asset: MixedAddress,
      pub amount: TokenAmount,      // Renamed from maxAmountRequired
      pub pay_to: MixedAddress,
      pub max_timeout_seconds: u64,
      pub extra: Option<serde_json::Value>,
  }
  ```
  - **File:** `src/types_v2.rs:31-80`

- [ ] **Task 2.2.2:** Implement `From<PaymentRequirements>` for v1->v2 conversion
- [ ] **Task 2.2.3:** Implement `TryFrom<PaymentRequirementsV2>` for v2->v1 conversion

#### 2.3 PaymentPayload v2
- [ ] **Task 2.3.1:** Create `PaymentPayloadV2` struct
  ```rust
  pub struct PaymentPayloadV2 {
      pub x402_version: u8,  // Always 2
      pub resource: ResourceInfo,
      pub accepted: PaymentRequirementsV2,
      pub payload: ExactPaymentPayload,
      pub extensions: Option<HashMap<String, serde_json::Value>>,
  }
  ```
  - **File:** `src/types_v2.rs:81-130`

#### 2.4 Envelope Types for Dual Support
- [ ] **Task 2.4.1:** Create `PaymentPayloadEnvelope` enum
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(untagged)]
  pub enum PaymentPayloadEnvelope {
      V1(PaymentPayload),
      V2(PaymentPayloadV2),
  }
  ```
  - **File:** `src/types_v2.rs:131-170`

- [ ] **Task 2.4.2:** Implement version detection logic
  - Check `x402_version` field value
  - V1: `x402_version: { major: 1, minor: 0 }`
  - V2: `x402_version: 2`

#### 2.5 Request/Response Types
- [ ] **Task 2.5.1:** Create `VerifyRequestEnvelope` for dual support
- [ ] **Task 2.5.2:** Create `SettleRequestEnvelope` for dual support
- [ ] **Task 2.5.3:** Create `PaymentRequiredResponse` struct (402 response)

#### 2.6 Extensions System
- [ ] **Task 2.6.1:** Create `Extension` trait for extensibility
- [ ] **Task 2.6.2:** Create `ExtensionRegistry` for extension lookup

#### 2.7 Serde Configuration
- [ ] **Task 2.7.1:** Configure `#[serde(rename_all = "camelCase")]` for v2 types
- [ ] **Task 2.7.2:** Add `#[serde(skip_serializing_if = "Option::is_none")]` for optional fields

### Deliverables
- `src/types_v2.rs` - New 500-line module
- Envelope types for seamless v1/v2 handling
- Full Serde support with proper casing

---

## Phase 3: Handler Updates
**Duration:** Week 3-4
**Agent:** Default + aegis-rust-architect (if complex)
**Files:** `src/handlers.rs`, `src/facilitator_local.rs`

### Overview
Update HTTP handlers to support both v1 and v2 protocols simultaneously.

### Tasks

#### 3.1 Version Detection
- [ ] **Task 3.1.1:** Add `detect_protocol_version()` function
  ```rust
  fn detect_protocol_version(payload: &serde_json::Value) -> ProtocolVersion {
      match payload.get("x402Version") {
          Some(Value::Number(n)) if n.as_u64() == Some(2) => ProtocolVersion::V2,
          _ => ProtocolVersion::V1,
      }
  }
  ```
  - **File:** `src/handlers.rs`

#### 3.2 Verify Endpoint Updates
- [ ] **Task 3.2.1:** Update `post_verify()` to accept envelope type
- [ ] **Task 3.2.2:** Route to v1 or v2 verification based on version
- [ ] **Task 3.2.3:** Return response in same version as request
- [ ] **Task 3.2.4:** Check feature flags before v2 processing

#### 3.3 Settle Endpoint Updates
- [ ] **Task 3.3.1:** Update `post_settle()` to accept envelope type
- [ ] **Task 3.3.2:** Route to v1 or v2 settlement based on version
- [ ] **Task 3.3.3:** Return response in same version as request
- [ ] **Task 3.3.4:** Check feature flags before v2 processing
- [ ] **Task 3.3.5:** Increment/decrement in-flight counter

#### 3.4 Supported Endpoint Updates
- [ ] **Task 3.4.1:** Update `/supported` response structure
  - Include CAIP-2 identifiers alongside legacy names
  - Add `extensions` field for available extensions
  - Add `signers` field per spec
  - **File:** `src/handlers.rs::get_supported()`

#### 3.5 Header Support
- [ ] **Task 3.5.1:** Add header parsing for `PAYMENT-SIGNATURE` (v2)
- [ ] **Task 3.5.2:** Add header parsing for `PAYMENT-REQUIRED` (v2)
- [ ] **Task 3.5.3:** Maintain backward compatibility with `X-PAYMENT` (v1)

#### 3.6 Logging Updates
- [ ] **Task 3.6.1:** Add structured logging for protocol version
  ```rust
  tracing::info!(
      protocol_version = %version,
      network = %network.to_caip2(),
      "Processing settlement request"
  );
  ```
  - No emojis in logs (per CLAUDE.md)

### Deliverables
- Dual v1/v2 support in all handlers
- Protocol version tracking in logs
- Backward compatible header handling
- Feature flag integration

---

## Phase 4: Testing & Quality
**Duration:** Week 4-5
**Agent:** Default
**Files:** `tests/`, `tests/integration/`

### Overview
Comprehensive testing to ensure dual-support works correctly across all networks.

### Tasks

#### 4.1 Unit Tests
- [ ] **Task 4.1.1:** Test CAIP-2 <-> Network conversion (all 20+ networks)
- [ ] **Task 4.1.2:** Test v2 type serialization/deserialization
- [ ] **Task 4.1.3:** Test envelope type version detection
- [ ] **Task 4.1.4:** Test PaymentRequirements v1<->v2 conversion

#### 4.2 Integration Tests
- [ ] **Task 4.2.1:** Create `tests/integration/test_v2_verify.py`
- [ ] **Task 4.2.2:** Create `tests/integration/test_v2_settle.py`
- [ ] **Task 4.2.3:** Test v1 backward compatibility (must not break)
- [ ] **Task 4.2.4:** Test mixed v1/v2 traffic simulation

#### 4.3 Network-Specific Tests
- [ ] **Task 4.3.1:** Test CAIP-2 for all EVM networks
- [ ] **Task 4.3.2:** Test CAIP-2 for Solana (genesis hash)
- [ ] **Task 4.3.3:** Test CAIP-2 for NEAR (near:mainnet, near:testnet)
- [ ] **Task 4.3.4:** Test CAIP-2 for Stellar (stellar:pubnet, stellar:testnet)
- [ ] **Task 4.3.5:** Test CAIP-2 for Fogo (fogo:mainnet, fogo:testnet)

#### 4.4 Safety Tests
- [ ] **Task 4.4.1:** Test graceful shutdown with in-flight transactions
- [ ] **Task 4.4.2:** Test feature flag disable/enable
- [ ] **Task 4.4.3:** Test nonce safety with concurrent requests
- [ ] **Task 4.4.4:** Test deep health check validation

### Deliverables
- 95%+ test coverage for v2 types
- Integration tests for v2 protocol
- Regression tests for v1 compatibility
- Safety mechanism tests

---

## Phase 5: Deployment (Blue-Green)
**Duration:** Week 5-6
**Agent:** terraform-aws-architect
**Files:** `terraform/`, `scripts/`

### Overview
Deploy v2-enabled facilitator using blue-green deployment with instant rollback capability.

### Tasks

#### 5.1 Pre-Deployment Checklist
- [ ] **Task 5.1.1:** Verify Phase 0 infrastructure is deployed
- [ ] **Task 5.1.2:** Verify instant rollback script works
- [ ] **Task 5.1.3:** Verify auto-rollback Lambda is active
- [ ] **Task 5.1.4:** Notify team of deployment window

#### 5.2 Build & Push
- [ ] **Task 5.2.1:** Update `Cargo.toml` version to `2.0.0`
- [ ] **Task 5.2.2:** Build Docker image
  ```bash
  ./scripts/build-and-push.sh v2.0.0
  ```
- [ ] **Task 5.2.3:** Tag release in git
  ```bash
  git tag -a v2.0.0 -m "x402 Protocol v2 Support"
  git push origin v2.0.0
  ```

#### 5.3 Shadow Testing (1-2 days)
- [ ] **Task 5.3.1:** Deploy v2 to green service with shadow mode
  ```bash
  # Green runs v2 with SHADOW_V2_ENABLED=true
  # All traffic still goes to blue (v1)
  # Green processes shadow copies
  ```
- [ ] **Task 5.3.2:** Monitor shadow logs for v1/v2 discrepancies
- [ ] **Task 5.3.3:** Fix any discrepancies found
- [ ] **Task 5.3.4:** Verify shadow success rate > 99.9%

#### 5.4 Gradual Traffic Shift
- [ ] **Task 5.4.1:** Shift 1% traffic to green
  ```bash
  ./scripts/shift-traffic.sh 1
  ```
  - Wait 30 minutes, monitor errors

- [ ] **Task 5.4.2:** Shift 10% traffic to green
  ```bash
  ./scripts/shift-traffic.sh 10
  ```
  - Wait 2 hours, monitor errors

- [ ] **Task 5.4.3:** Shift 50% traffic to green
  ```bash
  ./scripts/shift-traffic.sh 50
  ```
  - Wait 4 hours, monitor errors

- [ ] **Task 5.4.4:** Shift 100% traffic to green
  ```bash
  ./scripts/shift-traffic.sh 100
  ```
  - Monitor for 24 hours

#### 5.5 Post-Deployment Verification
- [ ] **Task 5.5.1:** Verify `/supported` includes CAIP-2 identifiers
- [ ] **Task 5.5.2:** Test v1 payment still works
- [ ] **Task 5.5.3:** Test v2 payment works
- [ ] **Task 5.5.4:** Check CloudWatch metrics dashboard
- [ ] **Task 5.5.5:** Verify no nonce errors in logs
- [ ] **Task 5.5.6:** Verify no settlement failures

#### 5.6 Cleanup
- [ ] **Task 5.6.1:** After 48 hours stable, scale down blue service
- [ ] **Task 5.6.2:** Keep blue task definition for emergency rollback
- [ ] **Task 5.6.3:** Update documentation with new version

### Deliverables
- v2.0.0 deployed via blue-green
- Shadow testing completed
- Gradual traffic shift (1% -> 10% -> 50% -> 100%)
- 24-hour stability verification
- Rollback procedure tested

---

## Phase 6: Migration & Deprecation
**Duration:** Month 2-6
**Agent:** Default
**Scope:** Documentation, monitoring, client communication

### Overview
6-month migration period where both v1 and v2 are supported, with gradual v1 deprecation.

### Tasks

#### 6.1 Migration Monitoring
- [ ] **Task 6.1.1:** Set up weekly v1/v2 traffic reports
- [ ] **Task 6.1.2:** Track CAIP-2 parsing error rate
- [ ] **Task 6.1.3:** Alert on v1-only clients (for outreach)

#### 6.2 Client Communication
- [ ] **Task 6.2.1:** Update API documentation for v2
- [ ] **Task 6.2.2:** Publish v2 migration guide
- [ ] **Task 6.2.3:** Notify existing clients of deprecation timeline

#### 6.3 v1 Deprecation (Month 6)
- [ ] **Task 6.3.1:** Add deprecation warning to v1 responses
- [ ] **Task 6.3.2:** Create v2.1.0 with v1 deprecated (not removed)
- [ ] **Task 6.3.3:** Create v3.0.0 with v1 removed (future)

#### 6.4 Documentation Updates
- [ ] **Task 6.4.1:** Update CLAUDE.md with v2 architecture
- [ ] **Task 6.4.2:** Update CHANGELOG.md with v2 release notes
- [ ] **Task 6.4.3:** Archive v1 documentation

#### 6.5 Post-Migration Cleanup
- [ ] **Task 6.5.1:** Disable blue-green (single service mode)
- [ ] **Task 6.5.2:** Revert to cached nonce mode (performance)
- [ ] **Task 6.5.3:** Remove shadow testing code
- [ ] **Task 6.5.4:** Remove v1 code paths (v3.0.0)

### Deliverables
- 95%+ v2 adoption by Month 6
- Clear deprecation communication
- Documentation fully updated
- Infrastructure optimized post-migration

---

## Risk Matrix (Updated)

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| **Nonce collision during deployment** | High (if rolling) | Critical | Blue-green deployment, fresh nonce fetch |
| **In-flight tx killed during deploy** | Medium | High | Graceful shutdown, 120s drain |
| **v2 bug causes settlement failure** | Medium | Critical | Shadow testing, auto-rollback, feature flags |
| CAIP-2 parsing bugs | Medium | High | Extensive unit tests, CloudWatch alarms |
| v1 regression | Low | Critical | Full v1 test suite, gradual traffic shift |
| Genesis hash mismatch | Low | Medium | Hardcoded constants, validation on startup |
| Client adoption slow | Medium | Low | 6-month deprecation window |
| Performance degradation | Low | Medium | Benchmarks before/after, rollback ready |

---

## Dependencies & Critical Path

```
Phase 0 (Safety) ──────────────────────────────────────────────┐
        │                                                      │
        v                                                      │
Phase 1 (CAIP-2) ──────┐                                       │
                       ├──> Phase 3 (Handlers) ──> Phase 4 ────┼──> Phase 5 (Deploy)
Phase 2 (v2 Types) ────┘                                       │           │
                                                               │           │
        ┌──────────────────────────────────────────────────────┘           │
        │                                                                  │
        v                                                                  v
   [Safety Infra Ready]                                              Phase 6 (Migration)
```

**Critical Path:** Phase 0 -> Phase 1 -> Phase 3 -> Phase 4 -> Phase 5

**PHASE 0 IS MANDATORY BEFORE ANY OTHER PHASE**

---

## Resource Requirements (Updated)

### Development Time
| Phase | Estimated Hours | Agent |
|-------|----------------|-------|
| **Phase 0** | **20-30 hours** | **aegis + terraform** |
| Phase 1 | 16-24 hours | aegis-rust-architect |
| Phase 2 | 12-18 hours | aegis-rust-architect |
| Phase 3 | 8-12 hours | Default |
| Phase 4 | 12-16 hours | Default |
| Phase 5 | 8-12 hours | terraform-aws-architect |
| Phase 6 | 8-12 hours | Default |
| **Total** | **84-124 hours** | |

### Infrastructure Cost
- Current: ~$44.60/month
- During deployment (blue-green active): ~$95/month
- After v2 stable: ~$55/month (+CloudWatch, can disable green)
- Post-migration (single service): ~$50/month

---

## Quick Reference - CAIP-2 Mappings

### EVM Networks (eip155:{chainId})
```
Base Mainnet:       eip155:8453
Base Sepolia:       eip155:84532
Ethereum:           eip155:1
Ethereum Sepolia:   eip155:11155111
Arbitrum:           eip155:42161
Arbitrum Sepolia:   eip155:421614
Optimism:           eip155:10
Optimism Sepolia:   eip155:11155420
Polygon:            eip155:137
Polygon Amoy:       eip155:80002
Avalanche:          eip155:43114
Avalanche Fuji:     eip155:43113
Celo:               eip155:42220
Celo Sepolia:       eip155:44787
HyperEVM:           eip155:999
HyperEVM Testnet:   eip155:333
Unichain:           eip155:130
Monad:              eip155:143
```

### Non-EVM Networks (custom namespaces)
```
Solana Mainnet:     solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp
Solana Devnet:      solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1
NEAR Mainnet:       near:mainnet
NEAR Testnet:       near:testnet
Stellar Pubnet:     stellar:pubnet
Stellar Testnet:    stellar:testnet
Fogo Mainnet:       fogo:mainnet
Fogo Testnet:       fogo:testnet
```

---

## Emergency Procedures

### Instant Rollback (< 5 seconds)
```bash
# If anything goes wrong during deployment:
./scripts/instant-rollback.sh

# This immediately:
# 1. Shifts 100% traffic to blue (v1)
# 2. Sends alert to team
# 3. Logs rollback reason
```

### Manual Traffic Shift
```bash
# Shift to specific percentage
./scripts/shift-traffic.sh 0    # 100% blue (v1)
./scripts/shift-traffic.sh 50   # 50/50 split
./scripts/shift-traffic.sh 100  # 100% green (v2)
```

### Disable v2 Without Rollback
```bash
# If v2 has issues but v1 traffic is fine:
# Use feature flags to disable v2 processing
aws ecs update-service ... --environment "V2_ENABLED=false"
```

### Check In-Flight Transactions
```bash
# Before any deployment action:
curl https://facilitator.ultravioletadao.xyz/metrics | grep in_flight
```

---

## Related Documents

- `docs/X402_V2_ANALYSIS.md` - Original protocol analysis
- `docs/X402_V2_TYPE_SYSTEM_DESIGN.md` - Detailed Rust type system
- `docs/X402_V2_INFRASTRUCTURE_ANALYSIS.md` - AWS infrastructure details
- `docs/X402_V2_DEPLOYMENT_RUNBOOK.md` - Step-by-step deployment guide
- `docs/X402_V2_INFRASTRUCTURE_SUMMARY.md` - Executive summary
- `terraform/environments/production/cloudwatch-v2-metrics.tf` - CloudWatch config
- `terraform/environments/production/blue-green.tf` - Blue-green infrastructure (TODO)
- `terraform/environments/production/TERRAFORM_CHANGES_V2.md` - Terraform quick reference

---

**Document Status:** APPROVED FOR IMPLEMENTATION (v2.0 - Zero-Downtime Edition)
**Created By:** Multi-Agent Collaboration + Gemini Critical Safety Review
**Date:** 2025-12-11
**Safety Review:** PASSED

---

```
============================================================================
REMEMBER: This is a production payment system.
When in doubt, DON'T deploy. Ask for review.
Millions of dollars depend on getting this right.
============================================================================
```

*"El mejor facilitador x402 v2 del mundo - sin perder un solo pago"* - Ultravioleta DAO
