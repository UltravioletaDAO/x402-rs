# x402 v2 Master Action Plan
## Ship Fast Edition

**Document Version:** 3.0 (Ship Now)
**Date:** 2025-12-11
**Approach:** Rolling deployment, fix forward, rollback if needed

---

## Strategy

```
Ship it. If it breaks, fix it or rollback to previous container.
No blue-green. No extra infrastructure. No waiting.
```

---

## Executive Summary

- **Total Tasks:** 51 tasks across 5 phases
- **Estimated Effort:** 60-88 hours of development
- **Infrastructure Cost Impact:** +$5/month (CloudWatch only)
- **Target Version:** v2.0.0
- **Deployment:** Rolling update (standard ECS)
- **Rollback:** Revert to previous task definition

---

## Phase Overview

| Phase | Name | Duration | Tasks | Priority |
|-------|------|----------|-------|----------|
| 1 | Foundation - CAIP-2 Types | Week 1-2 | 15 | CRITICAL |
| 2 | v2 Core Types | Week 2-3 | 14 | CRITICAL |
| 3 | Handler Updates | Week 3-4 | 12 | HIGH |
| 4 | Testing & Quality | Week 4-5 | 10 | HIGH |
| 5 | Deployment | Week 5 | 5 | CRITICAL |

---

## Phase 1: Foundation - CAIP-2 Types
**Duration:** Week 1-2
**Agent:** aegis-rust-architect
**Files:** `src/caip2.rs` (new), `src/network.rs`

### Tasks

#### 1.1 Create CAIP-2 Module
- [ ] **Task 1.1.1:** Create `src/caip2.rs` with `Namespace` enum
  - Variants: `Eip155`, `Solana`, `Near`, `Stellar`, `Fogo`
  - Implement `Display`, `FromStr`, `Serialize`, `Deserialize`

- [ ] **Task 1.1.2:** Implement `Caip2NetworkId` struct
  - Fields: `namespace: Namespace`, `reference: String`
  - Constructor with validation per namespace

- [ ] **Task 1.1.3:** Implement CAIP-2 parsing and formatting
  - `parse(s: &str) -> Result<Self, Caip2ParseError>`
  - `to_string() -> String`

- [ ] **Task 1.1.4:** Add custom Serde serialization

#### 1.2 CAIP-2 Error Handling
- [ ] **Task 1.2.1:** Create `Caip2ParseError` enum with `thiserror::Error`

#### 1.3 Network <-> CAIP-2 Conversion
- [ ] **Task 1.3.1:** Add `to_caip2()` method to `Network` enum
- [ ] **Task 1.3.2:** Add `from_caip2()` method to `Network` enum
- [ ] **Task 1.3.3:** Define CAIP-2 constants for all networks

#### 1.4 Genesis Hash Handling
- [ ] **Task 1.4.1:** Add Solana genesis hash constants
- [ ] **Task 1.4.2:** Add genesis hash validation for Solana

#### 1.5 Module Integration
- [ ] **Task 1.5.1:** Export `caip2` module from `src/lib.rs`
- [ ] **Task 1.5.2:** Add `mod caip2;` to `src/main.rs`
- [ ] **Task 1.5.3:** Update `Cargo.toml` if new dependencies needed

#### 1.6 Unit Tests
- [ ] **Task 1.6.1:** Test CAIP-2 parsing (valid cases)
- [ ] **Task 1.6.2:** Test CAIP-2 parsing (invalid cases)
- [ ] **Task 1.6.3:** Test Network <-> CAIP-2 round-trip for all networks

---

## Phase 2: v2 Core Types
**Duration:** Week 2-3
**Agent:** aegis-rust-architect
**Files:** `src/types_v2.rs` (new), `src/types.rs`

### Tasks

#### 2.1 ResourceInfo Type
- [ ] **Task 2.1.1:** Create `ResourceInfo` struct

#### 2.2 PaymentRequirements v2
- [ ] **Task 2.2.1:** Create `PaymentRequirementsV2` struct
- [ ] **Task 2.2.2:** Implement `From<PaymentRequirements>` for v1->v2 conversion
- [ ] **Task 2.2.3:** Implement `TryFrom<PaymentRequirementsV2>` for v2->v1 conversion

#### 2.3 PaymentPayload v2
- [ ] **Task 2.3.1:** Create `PaymentPayloadV2` struct

#### 2.4 Envelope Types for Dual Support
- [ ] **Task 2.4.1:** Create `PaymentPayloadEnvelope` enum (untagged serde)
- [ ] **Task 2.4.2:** Implement version detection logic

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

---

## Phase 3: Handler Updates
**Duration:** Week 3-4
**Agent:** Default + aegis-rust-architect (if complex)
**Files:** `src/handlers.rs`, `src/facilitator_local.rs`

### Tasks

#### 3.1 Version Detection
- [ ] **Task 3.1.1:** Add `detect_protocol_version()` function

#### 3.2 Verify Endpoint Updates
- [ ] **Task 3.2.1:** Update `post_verify()` to accept envelope type
- [ ] **Task 3.2.2:** Route to v1 or v2 verification based on version
- [ ] **Task 3.2.3:** Return response in same version as request

#### 3.3 Settle Endpoint Updates
- [ ] **Task 3.3.1:** Update `post_settle()` to accept envelope type
- [ ] **Task 3.3.2:** Route to v1 or v2 settlement based on version
- [ ] **Task 3.3.3:** Return response in same version as request

#### 3.4 Supported Endpoint Updates
- [ ] **Task 3.4.1:** Update `/supported` response structure
  - Include CAIP-2 identifiers alongside legacy names
  - Add `extensions` field
  - Add `signers` field per spec

#### 3.5 Header Support
- [ ] **Task 3.5.1:** Add header parsing for `PAYMENT-SIGNATURE` (v2)
- [ ] **Task 3.5.2:** Add header parsing for `PAYMENT-REQUIRED` (v2)
- [ ] **Task 3.5.3:** Maintain backward compatibility with `X-PAYMENT` (v1)

#### 3.6 Logging Updates
- [ ] **Task 3.6.1:** Add structured logging for protocol version

---

## Phase 4: Testing & Quality
**Duration:** Week 4-5
**Agent:** Default
**Files:** `tests/`, `tests/integration/`

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

---

## Phase 5: Deployment
**Duration:** Week 5
**Agent:** terraform-aws-architect
**Files:** `terraform/`, `scripts/`

### Tasks

#### 5.1 Build & Deploy
- [ ] **Task 5.1.1:** Update `Cargo.toml` version to `2.0.0`
- [ ] **Task 5.1.2:** Build and push Docker image
  ```bash
  ./scripts/build-and-push.sh v2.0.0
  ```
- [ ] **Task 5.1.3:** Deploy with rolling update
  ```bash
  aws ecs update-service --cluster facilitator-production \
    --service facilitator-production --force-new-deployment \
    --region us-east-2
  ```
- [ ] **Task 5.1.4:** Monitor logs for errors
  ```bash
  aws logs tail /ecs/facilitator-production --follow --region us-east-2
  ```
- [ ] **Task 5.1.5:** Verify endpoints work
  ```bash
  curl https://facilitator.ultravioletadao.xyz/health
  curl https://facilitator.ultravioletadao.xyz/supported | jq
  ```

---

## Rollback Procedure

If something breaks:

```bash
# Option 1: Revert to previous task definition
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:PREVIOUS_REVISION \
  --region us-east-2

# Option 2: Find previous revision number
aws ecs describe-services --cluster facilitator-production \
  --services facilitator-production --region us-east-2 \
  | jq '.services[0].taskDefinition'

# Option 3: Quick fix and redeploy
# Fix the code, rebuild, push, deploy again
```

---

## Quick Reference - CAIP-2 Mappings

### EVM Networks (eip155:{chainId})
```
Base Mainnet:       eip155:8453
Base Sepolia:       eip155:84532
Ethereum:           eip155:1
Arbitrum:           eip155:42161
Optimism:           eip155:10
Polygon:            eip155:137
Avalanche:          eip155:43114
Celo:               eip155:42220
HyperEVM:           eip155:999
```

### Non-EVM Networks
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

## Resource Requirements

| Phase | Estimated Hours | Agent |
|-------|----------------|-------|
| Phase 1 | 16-24 hours | aegis-rust-architect |
| Phase 2 | 12-18 hours | aegis-rust-architect |
| Phase 3 | 8-12 hours | Default |
| Phase 4 | 12-16 hours | Default |
| Phase 5 | 2-4 hours | terraform-aws-architect |
| **Total** | **50-74 hours** | |

---

## Related Documents

- `docs/X402_V2_ANALYSIS.md` - Protocol analysis
- `docs/X402_V2_TYPE_SYSTEM_DESIGN.md` - Rust type system design
- `docs/X402_V2_INFRASTRUCTURE_ANALYSIS.md` - AWS infrastructure (reference only)
- `terraform/environments/production/cloudwatch-v2-metrics.tf` - Optional monitoring

---

**Ship it.**
