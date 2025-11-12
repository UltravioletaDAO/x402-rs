# x402-compliance: Modular Compliance Screening Library
## Architecture Design Document

**Goal:** Create a reusable, plug-and-play compliance module that any x402-rs facilitator can integrate with minimal code changes.

**Status:** Design Phase
**Created:** 2025-11-10

---

## Vision

Build a standalone Rust crate `x402-compliance` that provides:
- ✅ Multi-jurisdictional sanctions screening (OFAC, UN, UK, EU, BIS)
- ✅ Structured compliance audit logging
- ✅ Blacklist/allowlist management
- ✅ Address extraction for EVM and Solana chains
- ✅ Simple integration API (3-5 lines of code to add to any facilitator)
- ✅ Configuration via environment variables or config file
- ✅ Zero dependencies on facilitator internals

### Use Cases

1. **Ultravioleta DAO x402-rs facilitator** (this repo)
2. **Upstream x402-rs facilitator** (github.com/x402-rs/x402-rs)
3. **Third-party facilitators** built on x402 protocol
4. **Payment gateways** using EIP-3009 or similar standards

---

## Project Structure

```
x402-rs/
├── Cargo.toml                    # Workspace manifest
├── crates/
│   ├── x402-compliance/          # ✨ NEW: Standalone compliance crate
│   │   ├── Cargo.toml
│   │   ├── README.md
│   │   ├── src/
│   │   │   ├── lib.rs            # Public API
│   │   │   ├── checker.rs        # ComplianceChecker trait + impl
│   │   │   ├── lists/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── ofac.rs       # OFAC SDN list loader
│   │   │   │   ├── un.rs         # UN Consolidated list loader
│   │   │   │   ├── uk.rs         # UK OFSI list loader
│   │   │   │   ├── eu.rs         # EU sanctions list loader
│   │   │   │   └── blacklist.rs  # Custom blacklist
│   │   │   ├── extractors/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── evm.rs        # Extract addresses from EIP-3009
│   │   │   │   └── solana.rs     # Extract addresses from Solana tx
│   │   │   ├── audit_logger.rs   # Structured logging
│   │   │   ├── config.rs         # Configuration loading
│   │   │   └── error.rs          # Error types
│   │   ├── config/               # Default config files
│   │   │   └── compliance.toml.example
│   │   └── tests/
│   │       ├── integration_tests.rs
│   │       └── fixtures/
│   ├── x402-axum/                # Existing: Axum middleware
│   └── x402-reqwest/             # Existing: Reqwest client
├── src/                          # Main facilitator
│   ├── main.rs
│   ├── facilitator.rs
│   ├── facilitator_local.rs      # ✏️ Modified: Use x402-compliance
│   ├── handlers.rs
│   └── ...
└── config/
    └── compliance.toml           # ✨ NEW: Compliance configuration
```

---

## Public API Design

### Core Types

```rust
// crates/x402-compliance/src/lib.rs

/// Re-export main types for easy importing
pub use checker::{ComplianceChecker, ComplianceCheckerBuilder};
pub use error::{ComplianceError, ScreeningDecision};
pub use audit_logger::{AuditLogger, ComplianceEvent};

/// Main screening result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreeningResult {
    pub decision: ScreeningDecision,
    pub payer_address: String,
    pub payee_address: String,
    pub matched_entities: Vec<MatchedEntity>,
    pub list_versions: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScreeningDecision {
    /// Transaction should be blocked
    Block { reason: String },
    /// Transaction needs manual review
    Review { reason: String },
    /// Transaction is clear to proceed
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedEntity {
    pub address: String,
    pub address_type: AddressType,  // Payer or Payee
    pub list_source: String,         // "OFAC_SDN", "UN_CONSOLIDATED", etc.
    pub entity_name: Option<String>,
    pub entity_id: Option<String>,
    pub program: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AddressType {
    Payer,
    Payee,
}
```

### ComplianceChecker Trait

```rust
// crates/x402-compliance/src/checker.rs

use async_trait::async_trait;

/// Main trait for compliance screening
#[async_trait]
pub trait ComplianceChecker: Send + Sync {
    /// Screen a payment transaction
    async fn screen_payment(
        &self,
        payer: &str,
        payee: &str,
        context: &TransactionContext,
    ) -> Result<ScreeningResult, ComplianceError>;

    /// Screen a single address
    async fn screen_address(&self, address: &str) -> Result<ScreeningDecision, ComplianceError>;

    /// Check if a specific list is loaded
    fn is_list_enabled(&self, list_name: &str) -> bool;

    /// Get metadata about loaded lists
    fn list_metadata(&self) -> HashMap<String, ListMetadata>;

    /// Reload/refresh sanctions lists
    async fn reload_lists(&mut self) -> Result<(), ComplianceError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionContext {
    pub amount: String,
    pub currency: String,
    pub network: String,
    pub transaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListMetadata {
    pub name: String,
    pub enabled: bool,
    pub record_count: usize,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub checksum: Option<String>,
    pub source_url: String,
}
```

### Builder Pattern for Easy Configuration

```rust
// crates/x402-compliance/src/checker.rs

pub struct ComplianceCheckerBuilder {
    ofac_enabled: bool,
    un_enabled: bool,
    uk_enabled: bool,
    eu_enabled: bool,
    blacklist_path: Option<PathBuf>,
    config_path: Option<PathBuf>,
    audit_logger: Option<Arc<AuditLogger>>,
}

impl ComplianceCheckerBuilder {
    pub fn new() -> Self {
        Self {
            ofac_enabled: true,  // Default: OFAC enabled
            un_enabled: false,
            uk_enabled: false,
            eu_enabled: false,
            blacklist_path: None,
            config_path: None,
            audit_logger: None,
        }
    }

    /// Enable OFAC SDN screening
    pub fn with_ofac(mut self, enabled: bool) -> Self {
        self.ofac_enabled = enabled;
        self
    }

    /// Enable UN Consolidated List
    pub fn with_un(mut self, enabled: bool) -> Self {
        self.un_enabled = enabled;
        self
    }

    /// Enable UK OFSI List
    pub fn with_uk(mut self, enabled: bool) -> Self {
        self.uk_enabled = enabled;
        self
    }

    /// Enable EU Sanctions List
    pub fn with_eu(mut self, enabled: bool) -> Self {
        self.eu_enabled = enabled;
        self
    }

    /// Load custom blacklist from file
    pub fn with_blacklist(mut self, path: impl Into<PathBuf>) -> Self {
        self.blacklist_path = Some(path.into());
        self
    }

    /// Load configuration from TOML file
    pub fn with_config_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Add structured audit logger
    pub fn with_audit_logger(mut self, logger: Arc<AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    /// Build the compliance checker
    pub async fn build(self) -> Result<Box<dyn ComplianceChecker>, ComplianceError> {
        // Load config if provided
        let config = if let Some(path) = self.config_path {
            Config::from_file(path)?
        } else {
            Config::from_env()?
        };

        // Load sanctions lists
        let mut lists = Vec::new();

        if self.ofac_enabled || config.lists.ofac.enabled {
            let ofac = lists::OfacList::load(&config.lists.ofac).await?;
            lists.push(Box::new(ofac) as Box<dyn SanctionsList>);
        }

        if self.un_enabled || config.lists.un.enabled {
            let un = lists::UnList::load(&config.lists.un).await?;
            lists.push(Box::new(un) as Box<dyn SanctionsList>);
        }

        // ... similar for UK, EU ...

        // Load blacklist if provided
        let blacklist = if let Some(path) = self.blacklist_path {
            Some(lists::Blacklist::from_file(path)?)
        } else if let Some(path) = config.blacklist_path {
            Some(lists::Blacklist::from_file(path)?)
        } else {
            None
        };

        // Create audit logger
        let audit_logger = self.audit_logger.unwrap_or_else(|| {
            Arc::new(AuditLogger::new(config.audit_logging))
        });

        Ok(Box::new(MultiListChecker {
            lists,
            blacklist,
            audit_logger,
            config,
        }))
    }
}
```

---

## Integration Examples

### Example 1: Minimal Integration (3 lines)

```rust
// In facilitator_local.rs

use x402_compliance::{ComplianceCheckerBuilder, ScreeningDecision};

// In main.rs or initialization
let compliance_checker = ComplianceCheckerBuilder::new()
    .with_ofac(true)
    .build()
    .await?;

// In verify() function
let result = compliance_checker.screen_payment(
    &payer_address,
    &payee_address,
    &TransactionContext {
        amount: amount.to_string(),
        currency: "USDC".to_string(),
        network: "base-mainnet".to_string(),
        transaction_id: None,
    }
).await?;

match result.decision {
    ScreeningDecision::Block { reason } => {
        return Err(FacilitatorLocalError::ComplianceViolation(reason));
    }
    ScreeningDecision::Review { reason } => {
        tracing::warn!("Payment requires manual review: {}", reason);
        // Could queue for review or block depending on policy
    }
    ScreeningDecision::Clear => {
        // Continue with payment processing
    }
}
```

### Example 2: Full Configuration with All Lists

```rust
// config/compliance.toml
[lists.ofac]
enabled = true
path = "config/ofac_addresses.json"
auto_update = true
update_interval_hours = 24

[lists.un]
enabled = true
path = "config/un_consolidated.json"
auto_update = true

[lists.uk]
enabled = true
path = "config/uk_ofsi.json"

[lists.eu]
enabled = true
path = "config/eu_sanctions.json"

[blacklist]
enabled = true
path = "config/blacklist.json"

[audit_logging]
enabled = true
target = "compliance_audit"
include_clear_transactions = false  # Only log BLOCK/REVIEW

[fail_mode]
on_list_load_error = "closed"  # or "open"
on_screening_error = "closed"

// In main.rs
let compliance_checker = ComplianceCheckerBuilder::new()
    .with_config_file("config/compliance.toml")
    .build()
    .await?;
```

### Example 3: Using Address Extractors

```rust
use x402_compliance::extractors::{EvmExtractor, SolanaExtractor};

// For EVM payments
match &payload {
    ExactPaymentPayload::Evm(evm_payload) => {
        let (payer, payee) = EvmExtractor::extract_addresses(evm_payload)?;

        let result = compliance_checker.screen_payment(
            &payer,
            &payee,
            &context,
        ).await?;
    }

    ExactPaymentPayload::Solana(solana_payload) => {
        let (payer, payee) = SolanaExtractor::extract_addresses(solana_payload)?;

        let result = compliance_checker.screen_payment(
            &payer,
            &payee,
            &context,
        ).await?;
    }
}
```

---

## Address Extractors Module

Separate module for parsing chain-specific transaction formats:

```rust
// crates/x402-compliance/src/extractors/evm.rs

use crate::error::ComplianceError;

pub struct EvmExtractor;

impl EvmExtractor {
    /// Extract payer and payee addresses from EIP-3009 authorization
    pub fn extract_addresses(
        authorization: &EvmAuthorization,  // Generic type, not tied to facilitator
    ) -> Result<(String, String), ComplianceError> {
        let payer = format!("{:?}", authorization.from);
        let payee = format!("{:?}", authorization.to);
        Ok((payer, payee))
    }
}

// crates/x402-compliance/src/extractors/solana.rs

pub struct SolanaExtractor;

impl SolanaExtractor {
    /// Extract payer and payee addresses from Solana transaction
    pub fn extract_addresses(
        transaction_base64: &str,
    ) -> Result<(String, String), ComplianceError> {
        use base64::{Engine as _, engine::general_purpose};
        use solana_sdk::transaction::Transaction;

        // Decode base64
        let tx_bytes = general_purpose::STANDARD
            .decode(transaction_base64)
            .map_err(|e| ComplianceError::AddressExtraction(e.to_string()))?;

        // Deserialize transaction
        let transaction: Transaction = bincode::deserialize(&tx_bytes)
            .map_err(|e| ComplianceError::AddressExtraction(e.to_string()))?;

        // Extract addresses
        let payer = transaction.message.account_keys
            .get(0)
            .ok_or(ComplianceError::AddressExtraction("No payer found".to_string()))?
            .to_string();

        let payee = transaction.message.account_keys
            .get(1)
            .ok_or(ComplianceError::AddressExtraction("No payee found".to_string()))?
            .to_string();

        Ok((payer, payee))
    }
}
```

---

## Configuration Schema

```toml
# config/compliance.toml

[lists.ofac]
enabled = true
path = "config/ofac_addresses.json"
source_url = "https://sanctionslistservice.ofac.treas.gov/api/PublicationPreview/exports/ADVANCED.JSON"
auto_update = true
update_interval_hours = 24

[lists.un]
enabled = false  # Enable in Phase 2
path = "config/un_consolidated.json"
source_url = "https://www.un.org/securitycouncil/content/un-sc-consolidated-list"
auto_update = false

[lists.uk]
enabled = false  # Enable in Phase 2
path = "config/uk_ofsi.json"
source_url = "https://www.gov.uk/government/publications/financial-sanctions-consolidated-list-of-targets"
auto_update = false

[lists.eu]
enabled = false  # Enable in Phase 2
path = "config/eu_sanctions.json"
source_url = "https://data.europa.eu/data/datasets/consolidated-list-of-persons-groups-and-entities-subject-to-eu-financial-sanctions"
auto_update = false

[blacklist]
enabled = true
path = "config/blacklist.json"

[audit_logging]
enabled = true
target = "compliance_audit"
format = "json"  # or "text"
include_clear_transactions = false  # Only log BLOCK/REVIEW to reduce noise

[fail_mode]
on_list_load_error = "open"   # "open" or "closed"
on_screening_error = "open"   # "open" or "closed"

[screening]
# Future: Fuzzy matching, scoring thresholds, etc.
enable_fuzzy_matching = false
block_threshold = 0.92
review_threshold = 0.85
```

---

## Dependency Management

### x402-compliance Cargo.toml

```toml
[package]
name = "x402-compliance"
version = "0.1.0"
edition = "2021"
authors = ["Ultravioleta DAO <dev@ultravioletadao.xyz>"]
description = "Modular compliance screening for x402 payment facilitators"
license = "MIT OR Apache-2.0"
repository = "https://github.com/UltravioletaDAO/x402-rs"

[dependencies]
# Core
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
async-trait = "0.1"
thiserror = "1.0"

# Logging
tracing = "0.1"
chrono = { version = "0.4", features = ["serde"] }

# Hashing
sha2 = "0.10"

# Blockchain
solana-sdk = { version = "1.17", optional = true }  # Only if Solana support enabled
ethers = { version = "2.0", optional = true }       # Only if EVM support enabled

# HTTP (for auto-updates)
reqwest = { version = "0.11", features = ["json"], optional = true }
tokio = { version = "1", features = ["full"], optional = true }

# Encoding
base64 = "0.21"
bincode = "1.3"

[dev-dependencies]
tokio-test = "0.4"
mockito = "1.2"

[features]
default = ["evm", "ofac"]
evm = ["ethers"]
solana = ["solana-sdk"]
ofac = []
un = []
uk = []
eu = []
auto_update = ["reqwest", "tokio"]
```

### Main Facilitator Cargo.toml Changes

```toml
# In workspace Cargo.toml
[workspace]
members = [
    ".",
    "crates/x402-axum",
    "crates/x402-reqwest",
    "crates/x402-compliance",  # ✨ NEW
]

# In main Cargo.toml
[dependencies]
x402-compliance = { path = "crates/x402-compliance", features = ["evm", "solana", "ofac"] }
```

---

## Migration Plan for Existing Code

### Step 1: Create Crate Structure
```bash
mkdir -p crates/x402-compliance/src/{lists,extractors}
touch crates/x402-compliance/Cargo.toml
touch crates/x402-compliance/src/{lib.rs,checker.rs,audit_logger.rs,config.rs,error.rs}
```

### Step 2: Move Existing Code
- Move `src/ofac_checker.rs` → `crates/x402-compliance/src/lists/ofac.rs`
- Move `src/blocklist.rs` → `crates/x402-compliance/src/lists/blacklist.rs`
- Adapt code to use generic types (no facilitator dependencies)

### Step 3: Create Public API
- Implement `ComplianceChecker` trait
- Create `ComplianceCheckerBuilder`
- Add address extractors

### Step 4: Update Facilitator Integration
- Replace direct OFAC/blacklist calls with `compliance_checker.screen_payment()`
- Update `facilitator_local.rs` to use new API
- Remove old `ofac_checker.rs` and `blocklist.rs` from main src/

---

## Testing Strategy

### Unit Tests (in x402-compliance crate)
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ofac_sanctioned_address() {
        let checker = ComplianceCheckerBuilder::new()
            .with_ofac(true)
            .build()
            .await
            .unwrap();

        let result = checker.screen_address("0x7F367cC41522cE07553e823bf3be79A889DEbe1B").await;
        assert!(matches!(result, Ok(ScreeningDecision::Block { .. })));
    }

    #[tokio::test]
    async fn test_clean_address() {
        let checker = ComplianceCheckerBuilder::new()
            .with_ofac(true)
            .build()
            .await
            .unwrap();

        let result = checker.screen_address("0x0000000000000000000000000000000000000000").await;
        assert!(matches!(result, Ok(ScreeningDecision::Clear)));
    }
}
```

### Integration Tests (in main facilitator)
```python
def test_compliance_blocks_sanctioned_payee():
    """End-to-end test: Payment to OFAC address should be blocked"""
    response = requests.post(
        "http://localhost:8080/verify",
        json={
            "authorization": {
                "from": "0xCleanAddress...",
                "to": "0x7F367cC41522cE07553e823bf3be79A889DEbe1B",  # Tornado Cash
                "value": "1000000"
            },
            "network": "base-mainnet",
            "scheme": "exact"
        }
    )
    assert response.status_code == 403
```

---

## Publishing and Distribution

### Option 1: Private Workspace Crate (Current)
- Keep `x402-compliance` as workspace member
- Use `path` dependency in Cargo.toml
- Share code by forking entire repo

### Option 2: Separate Public Crate (Future)
- Publish `x402-compliance` to crates.io
- Other facilitators can add: `x402-compliance = "0.1"`
- Maintain separate repo: github.com/UltravioletaDAO/x402-compliance

**Recommendation for Week 1:** Start with Option 1 (workspace crate), evaluate Option 2 after Phase 1 is complete.

---

## Benefits of Modular Design

### For Ultravioleta DAO
- ✅ Clean separation of concerns
- ✅ Easier testing in isolation
- ✅ Can update compliance logic without touching facilitator core
- ✅ Reusable across multiple services if needed

### For Upstream x402-rs
- ✅ Can adopt compliance module without custom Ultravioleta code
- ✅ No breaking changes to core facilitator
- ✅ Optional dependency (can disable if not needed)

### For Ecosystem
- ✅ Any x402 facilitator gets compliance for free
- ✅ Standardized sanctions screening across implementations
- ✅ Community can contribute new lists (Canada, Australia, etc.)
- ✅ Reduces regulatory risk for all participants

---

## Updated Week 1 Implementation Plan

With modular architecture, Week 1 tasks become:

### Task 1: Create x402-compliance Crate Structure (2 hours)
- [ ] Create crate directory structure
- [ ] Setup Cargo.toml with dependencies
- [ ] Define public API in lib.rs
- [ ] Create error types

### Task 2: Migrate OFAC Checker to Module (4 hours)
- [ ] Move `ofac_checker.rs` to `lists/ofac.rs`
- [ ] Remove facilitator dependencies
- [ ] Implement `SanctionsList` trait
- [ ] Add tests

### Task 3: Implement Address Extractors (4 hours)
- [ ] Create `extractors/evm.rs` for EIP-3009
- [ ] Create `extractors/solana.rs` for Solana transactions
- [ ] Add unit tests

### Task 4: Create ComplianceChecker Implementation (6 hours)
- [ ] Implement `ComplianceChecker` trait
- [ ] Create `MultiListChecker` struct
- [ ] Implement dual-screening (payer + payee)
- [ ] Add builder pattern

### Task 5: Add Structured Audit Logging (4 hours)
- [ ] Create `audit_logger.rs` module
- [ ] Define JSON event schema
- [ ] Integrate with tracing

### Task 6: Integrate into Facilitator (4 hours)
- [ ] Update `facilitator_local.rs` to use compliance module
- [ ] Replace direct OFAC calls with `compliance_checker.screen_payment()`
- [ ] Update error handling

### Task 7: Testing and Documentation (4 hours)
- [ ] Add integration tests
- [ ] Create README for x402-compliance crate
- [ ] Document API usage examples

**Total: 28 hours** (same as original plan, but with modular architecture)

---

## Next Steps

1. Review this architecture design
2. Approve modular approach
3. Begin implementation following updated Week 1 plan
4. Create GitHub issue template for community contributions (future)

---

**Status:** Awaiting approval to proceed with modular implementation
