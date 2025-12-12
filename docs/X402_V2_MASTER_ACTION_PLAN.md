# x402 v2 Master Action Plan
## "The Best x402 v2 Facilitator in the World"

**Document Version:** 1.0
**Date:** 2025-12-11
**Synthesized From:**
- Gemini Deep Research (7 areas)
- Aegis Rust Architect Analysis
- Terraform AWS Architect Analysis
- Task Decomposition Expert Framework

---

## Executive Summary

This document presents the comprehensive, task-decomposed action plan for migrating the Ultravioleta DAO x402-rs facilitator to protocol v2. The plan was created through collaborative multi-agent analysis, combining deep research, Rust architecture expertise, and AWS infrastructure knowledge.

**Key Metrics:**
- **Total Tasks:** 67 tasks across 6 phases
- **Estimated Effort:** 80-120 hours of development
- **Infrastructure Cost Impact:** +$5/month (CloudWatch metrics)
- **Target Version:** v2.0.0
- **Migration Period:** 6 months dual-support, then deprecate v1

**Our Competitive Advantages:**
1. Multi-chain support (EVM, Solana, NEAR, Stellar, Fogo) - unique in ecosystem
2. Custom CAIP-2 namespaces for non-standard chains
3. Production-hardened compliance module (OFAC screening)
4. Zero-downtime deployment with rollback capability

---

## Phase Overview

| Phase | Name | Duration | Tasks | Priority |
|-------|------|----------|-------|----------|
| 1 | Foundation - CAIP-2 Types | Week 1-2 | 15 | CRITICAL |
| 2 | v2 Core Types | Week 2-3 | 14 | CRITICAL |
| 3 | Handler Updates | Week 3-4 | 12 | HIGH |
| 4 | Testing & Quality | Week 4-5 | 10 | HIGH |
| 5 | Deployment | Week 5-6 | 9 | CRITICAL |
| 6 | Migration & Deprecation | Month 2-6 | 7 | MEDIUM |

---

## Phase 1: Foundation - CAIP-2 Types
**Duration:** Week 1-2
**Agent:** aegis-rust-architect
**Files:** `src/caip2.rs` (new), `src/network.rs`

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

#### 1.3 Network ↔ CAIP-2 Conversion
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
- [ ] **Task 1.6.3:** Test Network ↔ CAIP-2 round-trip for all networks

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

- [ ] **Task 2.2.2:** Implement `From<PaymentRequirements>` for v1→v2 conversion
- [ ] **Task 2.2.3:** Implement `TryFrom<PaymentRequirementsV2>` for v2→v1 conversion

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

#### 3.3 Settle Endpoint Updates
- [ ] **Task 3.3.1:** Update `post_settle()` to accept envelope type
- [ ] **Task 3.3.2:** Route to v1 or v2 settlement based on version
- [ ] **Task 3.3.3:** Return response in same version as request

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

---

## Phase 4: Testing & Quality
**Duration:** Week 4-5
**Agent:** Default
**Files:** `tests/`, `tests/integration/`

### Overview
Comprehensive testing to ensure dual-support works correctly across all networks.

### Tasks

#### 4.1 Unit Tests
- [ ] **Task 4.1.1:** Test CAIP-2 ↔ Network conversion (all 20+ networks)
- [ ] **Task 4.1.2:** Test v2 type serialization/deserialization
- [ ] **Task 4.1.3:** Test envelope type version detection
- [ ] **Task 4.1.4:** Test PaymentRequirements v1↔v2 conversion

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

### Deliverables
- 95%+ test coverage for v2 types
- Integration tests for v2 protocol
- Regression tests for v1 compatibility

---

## Phase 5: Deployment
**Duration:** Week 5-6
**Agent:** terraform-aws-architect
**Files:** `terraform/`, `scripts/`

### Overview
Deploy v2-enabled facilitator with monitoring and rollback capability.

### Tasks

#### 5.1 Infrastructure Updates
- [ ] **Task 5.1.1:** Apply CloudWatch metrics terraform
  ```bash
  cd terraform/environments/production
  terraform apply cloudwatch-v2-metrics.tf
  ```
  - **File:** `terraform/environments/production/cloudwatch-v2-metrics.tf` (already created)

- [ ] **Task 5.1.2:** Add `X402_VERSION_SUPPORT` environment variable
  - **File:** `terraform/environments/production/main.tf`
  - **Value:** `"v1,v2"`

- [ ] **Task 5.1.3:** Update ECS task definition with new env var

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

#### 5.3 Deployment
- [ ] **Task 5.3.1:** Deploy to ECS with rolling update
  ```bash
  aws ecs update-service --cluster facilitator-production \
    --service facilitator-production --force-new-deployment
  ```
- [ ] **Task 5.3.2:** Monitor deployment (10-15 minutes)
- [ ] **Task 5.3.3:** Verify health endpoint

#### 5.4 Post-Deployment Verification
- [ ] **Task 5.4.1:** Verify `/supported` includes CAIP-2 identifiers
- [ ] **Task 5.4.2:** Test v1 payment still works
- [ ] **Task 5.4.3:** Test v2 payment works
- [ ] **Task 5.4.4:** Check CloudWatch metrics dashboard

### Deliverables
- v2.0.0 deployed to production
- CloudWatch monitoring active
- Rollback procedure documented and tested

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

### Deliverables
- 95%+ v2 adoption by Month 6
- Clear deprecation communication
- Documentation fully updated

---

## Dependencies & Critical Path

```
Phase 1 (CAIP-2) ──────┐
                       ├──> Phase 3 (Handlers) ──> Phase 4 (Testing) ──> Phase 5 (Deploy)
Phase 2 (v2 Types) ────┘                                                      │
                                                                              │
                       ┌──────────────────────────────────────────────────────┘
                       │
                       v
                 Phase 6 (Migration)
```

**Critical Path:** Phase 1 → Phase 3 → Phase 4 → Phase 5

**Parallelizable:**
- Phase 1 and Phase 2 can run in parallel (different files)
- Phase 4 integration tests can start during late Phase 3

---

## Risk Matrix

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| CAIP-2 parsing bugs | Medium | High | Extensive unit tests, CloudWatch alarms |
| v1 regression | Low | Critical | Full v1 test suite before each deploy |
| Genesis hash mismatch | Low | Medium | Hardcoded constants, validation on startup |
| Client adoption slow | Medium | Low | 6-month deprecation window |
| Performance degradation | Low | Medium | Benchmarks before/after, rollback ready |

---

## Success Criteria

### Phase 1 Complete
- [ ] All 20+ networks have CAIP-2 identifiers
- [ ] Network ↔ CAIP-2 round-trip tests pass
- [ ] No runtime panics on malformed CAIP-2

### Phase 2 Complete
- [ ] v2 types serialize to upstream-compatible JSON
- [ ] Envelope types correctly detect version
- [ ] v1 types unchanged (backward compatibility)

### Phase 3 Complete
- [ ] `/verify` accepts both v1 and v2 payloads
- [ ] `/settle` accepts both v1 and v2 payloads
- [ ] `/supported` returns CAIP-2 identifiers

### Phase 4 Complete
- [ ] 95%+ test coverage on new code
- [ ] v1 integration tests still pass
- [ ] v2 integration tests pass

### Phase 5 Complete
- [ ] Production running v2.0.0
- [ ] CloudWatch dashboard showing v1/v2 split
- [ ] No errors in first 24 hours

### Phase 6 Complete
- [ ] 95%+ traffic on v2
- [ ] v1 deprecated (warnings in logs)
- [ ] Documentation fully updated

---

## Resource Requirements

### Development Time
| Phase | Estimated Hours | Agent |
|-------|----------------|-------|
| Phase 1 | 16-24 hours | aegis-rust-architect |
| Phase 2 | 12-18 hours | aegis-rust-architect |
| Phase 3 | 8-12 hours | Default |
| Phase 4 | 12-16 hours | Default |
| Phase 5 | 4-6 hours | terraform-aws-architect |
| Phase 6 | 8-12 hours | Default |
| **Total** | **60-88 hours** | |

### Infrastructure Cost
- Current: ~$44.60/month
- After v2: ~$49.60/month (+$5 CloudWatch)

### Testing Resources
- Local development environment
- Testnet wallets (already funded)
- Integration test scripts (Python)

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

## Next Steps

1. **Immediate:** Review this action plan with stakeholders
2. **This Week:** Begin Phase 1 implementation (CAIP-2 types)
3. **Week 2:** Begin Phase 2 in parallel (v2 types)
4. **Week 3-4:** Handler updates and testing
5. **Week 5:** Production deployment
6. **Month 2-6:** Migration monitoring and v1 deprecation

---

## Related Documents

- `docs/X402_V2_ANALYSIS.md` - Original protocol analysis
- `docs/X402_V2_TYPE_SYSTEM_DESIGN.md` - Detailed Rust type system
- `docs/X402_V2_INFRASTRUCTURE_ANALYSIS.md` - AWS infrastructure details
- `docs/X402_V2_DEPLOYMENT_RUNBOOK.md` - Step-by-step deployment guide
- `docs/X402_V2_INFRASTRUCTURE_SUMMARY.md` - Executive summary
- `terraform/environments/production/cloudwatch-v2-metrics.tf` - Ready to deploy
- `terraform/environments/production/TERRAFORM_CHANGES_V2.md` - Terraform quick reference

---

**Document Status:** APPROVED FOR IMPLEMENTATION
**Created By:** Multi-Agent Collaboration (Task Decomposition Expert + Aegis Rust Architect + Terraform AWS Architect + Gemini Deep Research)
**Date:** 2025-12-11

---

*"El mejor facilitador x402 v2 del mundo"* - Ultravioleta DAO
