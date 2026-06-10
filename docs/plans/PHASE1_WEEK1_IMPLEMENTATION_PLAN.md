# Phase 1 - Week 1: Immediate Compliance Fixes
## Implementation Master Plan

**Duration:** 28 hours (excluding fail-closed mode)
**Priority:** 🔴 CRITICAL
**Status:** Not Started
**Created:** 2025-11-10

---

## Overview

This plan implements the three critical compliance fixes identified in the compliance audit:
1. ✅ Screen payee (to) addresses in addition to payer (from) - **4 hours**
2. ✅ Add structured audit logging with compliance metadata - **8 hours**
3. ✅ Fix Solana address extraction for OFAC screening - **16 hours**

**NOTE:** Fail-closed mode implementation is intentionally excluded per user request.

---

## Task 1: Screen Payee (Beneficiary) Addresses

**Estimated Time:** 4 hours
**Priority:** 🔴 CRITICAL
**Status:** ⬜ Not Started

### Current Gap

**File:** `src/facilitator_local.rs` (lines 76-111)

Currently only the payer (`from`) address is screened:
```rust
let from_address = format!("{:?}", evm_payload.authorization.from);

// Check blacklist
if self.blacklist.is_blacklisted(&from_address) {
    return Err(FacilitatorLocalError::BlockedAddress(...));
}

// Check OFAC
if self.ofac_checker.is_sanctioned(&from_address) {
    return Err(FacilitatorLocalError::BlockedAddress(...));
}

// ❌ MISSING: No screening of "to" address (payee/beneficiary)
```

### Risk

- **Impact:** CRITICAL - Facilitating payments TO sanctioned entities
- **Compliance:** OFAC violations occur even if payer is clean but payee is sanctioned
- **Scenario:** Clean address sends USDC to North Korean wallet → facilitator settles it

### Implementation Steps

#### Step 1.1: Extract payee address from EVM payload (30 min)
- [ ] Open `src/facilitator_local.rs`
- [ ] Locate `verify()` function (around line 76)
- [ ] Add extraction of `to` address after `from` address extraction
- [ ] Format both addresses consistently

**Expected code:**
```rust
let from_address = format!("{:?}", evm_payload.authorization.from);
let to_address = format!("{:?}", evm_payload.authorization.to);
```

#### Step 1.2: Create dual-screening loop (1 hour)
- [ ] Replace individual checks with loop over both addresses
- [ ] Ensure error messages distinguish between payer vs payee hits
- [ ] Add address type to error context

**Expected code:**
```rust
for (address, address_type) in [
    (&from_address, "payer"),
    (&to_address, "payee")
] {
    // Check blacklist
    if self.blacklist.is_blacklisted(address) {
        tracing::error!(
            "BLACKLISTED {} address detected: {}",
            address_type,
            address
        );
        return Err(FacilitatorLocalError::BlockedAddress {
            address: address.clone(),
            reason: format!("Address is blacklisted ({})", address_type),
        });
    }

    // Check OFAC
    if self.ofac_checker.is_sanctioned(address) {
        tracing::error!(
            "OFAC SANCTIONED {} address detected: {}",
            address_type,
            address
        );
        return Err(FacilitatorLocalError::BlockedAddress {
            address: address.clone(),
            reason: format!("Address is on OFAC sanctions list ({})", address_type),
        });
    }
}
```

#### Step 1.3: Update error messages (30 min)
- [ ] Ensure `BlockedAddress` error includes address type context
- [ ] Verify error propagates to HTTP handler with correct status code (403)
- [ ] Check that error message doesn't leak sensitive entity details to end user

#### Step 1.4: Add tests for payee screening (1.5 hours)
- [ ] Create test file `tests/integration/test_payee_screening.py`
- [ ] Test case: Clean payer → Sanctioned payee (should BLOCK)
- [ ] Test case: Sanctioned payer → Clean payee (should BLOCK - already works)
- [ ] Test case: Sanctioned payer → Sanctioned payee (should BLOCK)
- [ ] Test case: Clean payer → Clean payee (should PASS)

**Expected test structure:**
```python
def test_block_payment_to_sanctioned_payee():
    """Should reject payment if payee is on OFAC list"""
    payload = {
        "authorization": {
            "from": "0xCleanAddress...",  # Not sanctioned
            "to": "0x7F367cC41522cE07553e823bf3be79A889DEbe1B",  # Tornado Cash (OFAC sanctioned)
            "value": "1000000",  # 1 USDC
            # ... other EIP-3009 fields
        },
        "network": "base-mainnet",
        "scheme": "exact"
    }

    response = requests.post("http://localhost:8080/verify", json=payload)
    assert response.status_code == 403
    assert "sanctioned" in response.json()["error"].lower()
    assert "payee" in response.json()["error"].lower()
```

#### Step 1.5: Manual verification (30 min)
- [ ] Start facilitator locally: `cargo run --release`
- [ ] Send test payment with sanctioned payee address
- [ ] Verify logs show "OFAC SANCTIONED payee address detected"
- [ ] Verify HTTP 403 response with appropriate error message
- [ ] Check that clean payee addresses still work

### Acceptance Criteria
- [x] Both `from` and `to` addresses are screened against blacklist
- [x] Both `from` and `to` addresses are screened against OFAC list
- [x] Error messages distinguish between payer vs payee violations
- [x] Tests pass for all 4 scenarios (clean/sanctioned combinations)
- [x] No false positives on legitimate transactions
- [x] Performance impact < 1ms (dual screening is still O(1) HashSet lookup)

---

## Task 2: Structured Audit Logging

**Estimated Time:** 8 hours
**Priority:** 🔴 CRITICAL
**Status:** ⬜ Not Started

### Current Gap

**Current logging:**
```rust
tracing::error!("OFAC SANCTIONED address detected: {}", from_address);
```

**Problems:**
- No transaction ID correlation
- No timestamp in structured format
- No entity details (name, program, list version)
- No payment context (amount, network, currency)
- Not compliance-grade for 5-year retention requirements
- Cannot export to SIEM/compliance database

### Implementation Steps

#### Step 2.1: Create audit logging module (2 hours)
- [ ] Create new file `src/audit_logger.rs`
- [ ] Define structured log event types
- [ ] Implement JSON serialization with serde
- [ ] Add log levels: BLOCK, REVIEW, INFO

**Expected structure:**
```rust
// src/audit_logger.rs

use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Serialize, Deserialize)]
pub struct ComplianceEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub decision: Decision,
    pub transaction_context: TransactionContext,
    pub screening_result: ScreeningResult,
    pub source_ip: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum EventType {
    SanctionsHit,
    BlacklistHit,
    CleanTransaction,
    ScreeningError,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Decision {
    Block,
    Review,
    Clear,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionContext {
    pub transaction_id: Option<String>,
    pub payer_address: String,
    pub payee_address: String,
    pub amount: String,
    pub currency: String,
    pub network: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScreeningResult {
    pub address_type: String,  // "payer" or "payee"
    pub matched_address: String,
    pub matched_entity: Option<EntityInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EntityInfo {
    pub name: Option<String>,
    pub entity_id: Option<String>,
    pub program: Option<String>,
    pub list_source: String,  // "OFAC_SDN", "blacklist"
    pub list_version: Option<String>,
    pub list_checksum: Option<String>,
}

pub struct AuditLogger {
    // Future: Could add async file writer, S3 uploader, SIEM connector
}

impl AuditLogger {
    pub fn new() -> Self {
        Self {}
    }

    pub fn log_compliance_event(&self, event: ComplianceEvent) {
        let json = serde_json::to_string(&event)
            .unwrap_or_else(|e| format!("{{\"error\": \"Failed to serialize: {}\"}}", e));

        match event.decision {
            Decision::Block => tracing::error!(target: "compliance_audit", "{}", json),
            Decision::Review => tracing::warn!(target: "compliance_audit", "{}", json),
            Decision::Clear => tracing::info!(target: "compliance_audit", "{}", json),
        }
    }
}
```

#### Step 2.2: Integrate audit logger into facilitator (2 hours)
- [ ] Add `audit_logger: Arc<AuditLogger>` to `FacilitatorLocal` struct
- [ ] Initialize in `main.rs` and pass to facilitator
- [ ] Update `verify()` function to call audit logger before/after screening

**Expected integration in `facilitator_local.rs`:**
```rust
// After screening check
if self.ofac_checker.is_sanctioned(address) {
    // Log structured compliance event
    let event = ComplianceEvent {
        timestamp: Utc::now(),
        event_type: EventType::SanctionsHit,
        decision: Decision::Block,
        transaction_context: TransactionContext {
            transaction_id: None,  // Could extract from payload if available
            payer_address: from_address.clone(),
            payee_address: to_address.clone(),
            amount: evm_payload.authorization.value.to_string(),
            currency: "USDC".to_string(),  // Could extract from network config
            network: format!("{:?}", network),
        },
        screening_result: ScreeningResult {
            address_type: address_type.to_string(),
            matched_address: address.clone(),
            matched_entity: Some(EntityInfo {
                name: None,  // TODO: Phase 2 - extract from SDN list
                entity_id: None,
                program: None,
                list_source: "OFAC_SDN".to_string(),
                list_version: self.ofac_checker.version(),
                list_checksum: self.ofac_checker.checksum(),
            }),
        },
        source_ip: None,  // Could extract from HTTP headers
    };

    self.audit_logger.log_compliance_event(event);

    return Err(FacilitatorLocalError::BlockedAddress { ... });
}
```

#### Step 2.3: Add metadata to OFAC checker (2 hours)
- [ ] Open `src/ofac_checker.rs`
- [ ] Add fields: `list_version: Option<String>`, `list_checksum: Option<String>`, `last_updated: Option<DateTime<Utc>>`
- [ ] Calculate SHA-256 checksum when loading `config/ofac_addresses.json`
- [ ] Extract version/date from OFAC JSON metadata (if available)
- [ ] Add public methods: `version()`, `checksum()`, `last_updated()`

**Expected additions:**
```rust
// In OfacChecker struct
pub struct OfacChecker {
    enabled: bool,
    sanctioned_addresses: HashSet<String>,
    list_version: Option<String>,
    list_checksum: Option<String>,
    last_updated: Option<DateTime<Utc>>,
}

impl OfacChecker {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;

        // Calculate SHA-256 checksum
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let checksum = format!("{:x}", hasher.finalize());

        // Parse addresses
        let addresses: Vec<String> = serde_json::from_str(&content)?;

        // Get file metadata for last_updated
        let metadata = std::fs::metadata(path)?;
        let last_updated = metadata.modified().ok()
            .map(|time| DateTime::<Utc>::from(time));

        Ok(Self {
            enabled: true,
            sanctioned_addresses: addresses.into_iter().collect(),
            list_version: Some("OFAC-SDN".to_string()),  // Could parse from JSON
            list_checksum: Some(checksum),
            last_updated,
        })
    }

    pub fn version(&self) -> Option<String> {
        self.list_version.clone()
    }

    pub fn checksum(&self) -> Option<String> {
        self.list_checksum.clone()
    }
}
```

#### Step 2.4: Add Cargo dependencies (15 min)
- [ ] Open `Cargo.toml`
- [ ] Add `chrono = { version = "0.4", features = ["serde"] }`
- [ ] Add `sha2 = "0.10"` for checksum calculation
- [ ] Run `cargo build` to verify dependencies resolve

#### Step 2.5: Test structured logging (1.5 hours)
- [ ] Start facilitator locally
- [ ] Trigger sanctioned address detection
- [ ] Verify JSON log output contains all required fields
- [ ] Parse JSON with `jq` to validate structure
- [ ] Check that timestamps are ISO 8601 format
- [ ] Verify checksums are SHA-256 hex strings

**Expected log output:**
```json
{
  "timestamp": "2025-11-10T19:30:45.123456Z",
  "event_type": "SanctionsHit",
  "decision": "Block",
  "transaction_context": {
    "transaction_id": null,
    "payer_address": "0x1234567890123456789012345678901234567890",
    "payee_address": "0x7F367cC41522cE07553e823bf3be79A889DEbe1B",
    "amount": "1000000",
    "currency": "USDC",
    "network": "base-mainnet"
  },
  "screening_result": {
    "address_type": "payee",
    "matched_address": "0x7f367cc41522ce07553e823bf3be79a889debe1b",
    "matched_entity": {
      "name": null,
      "entity_id": null,
      "program": null,
      "list_source": "OFAC_SDN",
      "list_version": "OFAC-SDN",
      "list_checksum": "a3f8b9c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9",
      "last_updated": "2025-11-10T08:00:00Z"
    }
  },
  "source_ip": null
}
```

#### Step 2.6: Documentation (30 min)
- [ ] Document audit log format in `docs/COMPLIANCE_AUDIT_LOGGING.md`
- [ ] Add example queries for common compliance scenarios
- [ ] Document retention requirements (5 years per FinCEN)
- [ ] Add instructions for exporting to SIEM/S3

### Acceptance Criteria
- [x] All compliance events logged in structured JSON format
- [x] Logs include transaction context (amount, network, addresses)
- [x] OFAC list metadata captured (version, checksum, last_updated)
- [x] Timestamps in ISO 8601 format
- [x] Logs distinguish between BLOCK/REVIEW/CLEAR decisions
- [x] Logs use dedicated tracing target `compliance_audit`
- [x] SHA-256 checksums calculated correctly
- [x] Documentation created for audit log format

---

## Task 3: Fix Solana Address Extraction

**Estimated Time:** 16 hours
**Priority:** 🔴 CRITICAL
**Status:** ⬜ Not Started

### Current Gap

**File:** `src/facilitator_local.rs` (lines 103-109)

```rust
ExactPaymentPayload::Solana(_solana_payload) => {
    // For Solana, we would need to parse the transaction to extract the signer
    // This is more complex and may require decoding the base64 transaction
    // For now, we'll skip Solana blacklist/OFAC checking in verify()
    // TODO: Implement Solana address extraction and blacklist check
    tracing::debug!("Skipping blacklist/OFAC check for Solana (not implemented)");
}
```

### Risk

- **Impact:** MEDIUM - Solana payments bypass all sanctions screening
- **Volume:** Currently low Solana usage, but growing
- **Compliance:** Regulatory violation if Solana becomes significant payment channel

### Implementation Steps

#### Step 3.1: Research Solana transaction structure (2 hours)
- [ ] Review Solana transaction format documentation
- [ ] Understand how EIP-3009-equivalent authorization works on Solana
- [ ] Identify where payer/payee addresses are encoded in transaction
- [ ] Review `src/chain/solana.rs` for existing parsing logic

**Key questions to answer:**
- What format is `solana_payload.transaction`? (base64? hex?)
- What Solana SDK types are already in use?
- Where are the `from` (signer) and `to` (recipient) pubkeys in the transaction?
- Is there an existing deserialization method we can leverage?

#### Step 3.2: Add Solana SDK dependency (30 min)
- [ ] Open `Cargo.toml`
- [ ] Check if `solana-sdk` is already a dependency (likely yes)
- [ ] If not, add `solana-sdk = "1.17"` (match version with existing Solana deps)
- [ ] Add `solana-transaction-status = "1.17"` if needed for transaction parsing
- [ ] Run `cargo build` to verify

#### Step 3.3: Implement Solana address extraction function (4 hours)
- [ ] Create helper function in `facilitator_local.rs` or `chain/solana.rs`
- [ ] Decode base64 transaction string
- [ ] Deserialize to `solana_sdk::transaction::Transaction`
- [ ] Extract signer public key (payer)
- [ ] Extract recipient public key from instruction data
- [ ] Return `(payer_pubkey, payee_pubkey)` as strings

**Expected function signature:**
```rust
fn extract_solana_addresses(
    transaction_base64: &str
) -> Result<(String, String), Box<dyn std::error::Error>> {
    // Decode base64
    use base64::{Engine as _, engine::general_purpose};
    let tx_bytes = general_purpose::STANDARD.decode(transaction_base64)?;

    // Deserialize transaction
    use solana_sdk::transaction::Transaction;
    let transaction: Transaction = bincode::deserialize(&tx_bytes)?;

    // Extract signer (payer) - first account key and fee payer
    let payer = transaction.message.account_keys
        .get(0)
        .ok_or("No payer account found")?
        .to_string();

    // Extract recipient - depends on instruction structure
    // For token transfers, this is typically in instruction accounts[1]
    let payee = transaction.message.account_keys
        .get(1)
        .ok_or("No recipient account found")?
        .to_string();

    Ok((payer, payee))
}
```

**Note:** The exact extraction logic depends on whether this is:
- Native SOL transfer
- SPL token transfer (USDC on Solana)
- Token-2022 program transfer

Need to analyze `chain/solana.rs` to understand which program is used.

#### Step 3.4: Integrate Solana screening into verify() (2 hours)
- [ ] Replace TODO block with actual address extraction
- [ ] Call `extract_solana_addresses()` to get payer and payee
- [ ] Screen both addresses against blacklist
- [ ] Screen both addresses against OFAC (list already includes Solana addresses)
- [ ] Handle extraction errors gracefully (log warning, optionally block)

**Expected code:**
```rust
ExactPaymentPayload::Solana(solana_payload) => {
    match extract_solana_addresses(&solana_payload.transaction) {
        Ok((payer, payee)) => {
            tracing::debug!("Extracted Solana addresses - payer: {}, payee: {}", payer, payee);

            // Screen both addresses
            for (address, address_type) in [(&payer, "payer"), (&payee, "payee")] {
                if self.blacklist.is_blacklisted(address) {
                    tracing::error!("BLACKLISTED Solana {} address: {}", address_type, address);
                    return Err(FacilitatorLocalError::BlockedAddress {
                        address: address.clone(),
                        reason: format!("Solana address is blacklisted ({})", address_type),
                    });
                }

                if self.ofac_checker.is_sanctioned(address) {
                    tracing::error!("OFAC SANCTIONED Solana {} address: {}", address_type, address);

                    // Log structured compliance event (if Task 2 completed)
                    // ... audit logging code ...

                    return Err(FacilitatorLocalError::BlockedAddress {
                        address: address.clone(),
                        reason: format!("Solana address is on OFAC sanctions list ({})", address_type),
                    });
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to extract Solana addresses for screening: {}", e);
            // Decision: Fail-open (allow) or fail-closed (reject)?
            // Recommendation: Log warning but allow, to avoid breaking Solana payments
            // Can revisit in Phase 2 with fail-closed configuration
        }
    }
}
```

#### Step 3.5: Verify OFAC list includes Solana addresses (1 hour)
- [ ] Open `config/ofac_addresses.json`
- [ ] Search for Solana-format addresses (base58, 32-44 characters)
- [ ] Identify at least one Solana address from OFAC SDN list
- [ ] Test that `ofac_checker.is_sanctioned()` works with Solana format
- [ ] Document how many Solana addresses are in current list

**Expected verification:**
```bash
# Check if OFAC list has Solana addresses
cat config/ofac_addresses.json | jq '.[]' | grep -E '^[1-9A-HJ-NP-Za-km-z]{32,44}$' | head -5

# Example Solana addresses from OFAC:
# "4WbLW..." (base58 format, ~44 chars)
```

#### Step 3.6: Test Solana address extraction (4 hours)
- [ ] Create test file `tests/integration/test_solana_screening.py`
- [ ] Generate test Solana transaction with known payer/payee
- [ ] Test case: Clean Solana payer → Clean payee (should PASS)
- [ ] Test case: Sanctioned Solana payer (should BLOCK)
- [ ] Test case: Clean payer → Sanctioned Solana payee (should BLOCK)
- [ ] Test case: Malformed transaction (should handle gracefully)

**Test approach:**
```python
# Need to generate a valid Solana transaction for testing
# Can use solana-py or similar library

from solders.keypair import Keypair
from solders.transaction import Transaction
from solders.message import Message
import base64

def create_test_solana_transaction(from_keypair, to_pubkey, amount):
    # Create a simple SOL transfer transaction
    # ... transaction creation logic ...

    # Serialize and encode to base64
    tx_bytes = bytes(transaction)
    tx_base64 = base64.b64encode(tx_bytes).decode('utf-8')
    return tx_base64

def test_solana_sanctioned_payer():
    # Use a known OFAC-sanctioned Solana address as payer
    sanctioned_address = "..."  # From OFAC list

    payload = {
        "transaction": create_test_solana_transaction(...),
        "network": "solana-mainnet",
        "scheme": "exact"
    }

    response = requests.post("http://localhost:8080/verify", json=payload)
    assert response.status_code == 403
    assert "sanctioned" in response.json()["error"].lower()
```

**Alternative if transaction generation is too complex:**
- Capture real Solana transaction from Solana devnet
- Use `solana-cli` to create and serialize transaction
- Hardcode base64 transaction in test with known addresses

#### Step 3.7: Error handling and edge cases (2 hours)
- [ ] Test with invalid base64 encoding
- [ ] Test with valid base64 but invalid transaction structure
- [ ] Test with transaction missing expected accounts
- [ ] Decide on fail-open vs fail-closed behavior for extraction errors
- [ ] Add metrics for Solana screening success/failure rates

#### Step 3.8: Documentation (30 min)
- [ ] Update `docs/COMPLIANCE_AUDIT_REPORT.md` to mark Solana TODO as resolved
- [ ] Document Solana address extraction logic
- [ ] Note any limitations (e.g., only supports specific instruction types)
- [ ] Add troubleshooting section for common Solana screening issues

### Acceptance Criteria
- [x] Solana transactions no longer skip OFAC/blacklist screening
- [x] Payer (signer) address extracted correctly from Solana transaction
- [x] Payee (recipient) address extracted correctly from Solana transaction
- [x] Both addresses screened against blacklist and OFAC list
- [x] Error handling for malformed transactions (graceful degradation)
- [x] Tests pass for clean and sanctioned Solana addresses
- [x] Logs show "Extracted Solana addresses" debug message
- [x] No breaking changes to existing Solana payment flow

### Known Challenges
- **Complexity:** Solana transaction structure is more complex than EVM
- **Instruction Parsing:** May need to parse specific program instructions (SPL Token vs native SOL)
- **Testing:** Generating valid Solana transactions for tests requires Solana SDK knowledge
- **Address Format:** Solana uses base58 vs EVM hex, ensure OFAC checker handles both

**Mitigation:** Start with simple native SOL transfers, expand to SPL tokens in Phase 2 if needed.

---

## Success Metrics

At the end of Week 1, we should have:

### Compliance Coverage
- [x] **100% transaction screening** - Both payer AND payee addresses screened
- [x] **Solana support** - No more screening bypass for Solana network
- [x] **Audit trail** - Structured JSON logs for all compliance events

### Performance
- [x] Screening overhead remains < 5ms per transaction
- [x] No degradation in throughput (still 100+ TPS capable)

### Testing
- [x] All new tests pass (payee screening, Solana extraction)
- [x] Existing integration tests still pass (no regressions)
- [x] Manual verification with real sanctioned addresses

### Documentation
- [x] Implementation details documented
- [x] Audit log format specified
- [x] Solana extraction logic explained

---

## Rollout Plan

### Local Testing
1. Implement all three tasks in development branch
2. Run full test suite: `cargo test && cd tests/integration && python -m pytest`
3. Manual testing with curl/Postman against local facilitator
4. Performance testing with k6 load tests

### Staging Deployment
1. Deploy to staging ECS environment
2. Smoke test with real (non-production) traffic
3. Verify audit logs appear in CloudWatch
4. Monitor for 24 hours

### Production Deployment
1. Deploy during low-traffic window
2. Monitor error rates and latency
3. Verify no increase in false positives
4. Check audit log volume and format

### Rollback Plan
If critical issues detected:
1. Revert to previous ECS task definition
2. Investigate logs for root cause
3. Fix in development branch
4. Re-test and re-deploy

---

## Dependencies and Prerequisites

### Software
- [x] Rust 1.82+ (edition 2021)
- [x] Python 3.9+ for integration tests
- [x] Docker for local testing
- [x] AWS CLI for ECS deployment

### Rust Crates (to be added)
- [ ] `chrono = "0.4"` with serde feature
- [ ] `sha2 = "0.10"` for checksums
- [ ] `base64 = "0.21"` (may already exist)
- [ ] `bincode` (may already exist for Solana)

### Knowledge Required
- Basic Rust programming
- Understanding of EIP-3009 payment flow
- Solana transaction structure (for Task 3)
- JSON serialization with serde

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| False positives blocking legitimate users | Medium | High | Extensive testing with known clean addresses |
| Solana extraction breaks existing flow | Medium | High | Graceful error handling (fail-open initially) |
| Performance degradation | Low | Medium | Benchmark before/after, dual screening is still O(1) |
| Missing Solana addresses in OFAC list | Low | Low | Verify list contains Solana format addresses |
| Audit logs too verbose | Low | Low | Use dedicated tracing target, can filter in prod |

---

## Next Steps After Week 1

Once Week 1 is complete, we proceed to:

**Week 2:** Multi-List Integration
- Add UN Consolidated List
- Add UK OFSI List
- Add EU Consolidated List

See `PHASE1_WEEK2_IMPLEMENTATION_PLAN.md` (to be created)

---

## Checklist Summary

### Task 1: Screen Payee Addresses (4 hours)
- [ ] Extract payee address from EVM payload
- [ ] Create dual-screening loop for payer + payee
- [ ] Update error messages with address type
- [ ] Add integration tests for payee screening
- [ ] Manual verification

### Task 2: Structured Audit Logging (8 hours)
- [ ] Create `src/audit_logger.rs` module
- [ ] Define structured event types with serde
- [ ] Integrate audit logger into facilitator
- [ ] Add metadata fields to OFAC checker (version, checksum)
- [ ] Add required Cargo dependencies
- [ ] Test JSON log output format
- [ ] Create documentation

### Task 3: Fix Solana Address Extraction (16 hours)
- [ ] Research Solana transaction structure
- [ ] Add/verify Solana SDK dependencies
- [ ] Implement address extraction function
- [ ] Integrate screening into verify() flow
- [ ] Verify OFAC list includes Solana addresses
- [ ] Create Solana screening tests
- [ ] Handle edge cases and errors
- [ ] Update documentation

---

**Status Legend:**
- ⬜ Not Started
- 🟦 In Progress
- ✅ Complete
- ❌ Blocked

**Last Updated:** 2025-11-10
