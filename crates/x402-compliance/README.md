# x402-compliance

Modular compliance screening library for x402 payment facilitators.

## Features

- Multi-jurisdictional sanctions screening (OFAC, UN, UK, EU)
- Custom blacklist/allowlist management
- Structured compliance audit logging
- Address extraction for EVM and Solana chains
- Simple plug-and-play integration
- Configuration via TOML or builder pattern

## Quick Start

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
x402-compliance = { path = "../x402-compliance", features = ["solana"] }
```

### Basic Usage

```rust
use x402_compliance::{ComplianceCheckerBuilder, ScreeningDecision, TransactionContext};

// Initialize compliance checker
let compliance_checker = ComplianceCheckerBuilder::new()
    .with_ofac(true)
    .with_blacklist("config/blacklist.json")
    .build()
    .await?;

// Screen a payment
let result = compliance_checker.screen_payment(
    &payer_address,
    &payee_address,
    &TransactionContext {
        amount: "1000.00".to_string(),
        currency: "USDC".to_string(),
        network: "base-mainnet".to_string(),
        transaction_id: None,
    }
).await?;

match result.decision {
    ScreeningDecision::Block { reason } => {
        // Reject the payment
        return Err(format!("Payment blocked: {}", reason));
    }
    ScreeningDecision::Review { reason } => {
        // Queue for manual review
        log::warn!("Payment requires review: {}", reason);
    }
    ScreeningDecision::Clear => {
        // Continue with payment processing
    }
}
```

### Using Address Extractors

```rust
use x402_compliance::extractors::{EvmExtractor, SolanaExtractor};

// EVM (Ethereum, Base, Polygon, etc.)
let (payer, payee) = EvmExtractor::extract_addresses(
    &evm_payload.authorization.from,
    &evm_payload.authorization.to
)?;

// Solana
let (payer, payee) = SolanaExtractor::extract_addresses(
    &solana_payload.transaction  // base64-encoded transaction
)?;
```

### Configuration File

Create `config/compliance.toml`:

```toml
[lists.ofac]
enabled = true
path = "config/ofac_addresses.json"
auto_update = false

[blacklist]
enabled = true
path = "config/blacklist.json"

[audit_logging]
enabled = true
target = "compliance_audit"
format = "json"
include_clear_transactions = false

[fail_mode]
on_list_load_error = "open"   # or "closed"
on_screening_error = "open"
```

Then load it:

```rust
let compliance_checker = ComplianceCheckerBuilder::new()
    .with_config_file("config/compliance.toml")
    .build()
    .await?;
```

## Features

- `default`: Enables OFAC screening
- `solana`: Adds Solana transaction parsing support
- `ofac`: OFAC SDN list support
- `un`: UN Consolidated List support (Phase 2)
- `uk`: UK OFSI list support (Phase 2)
- `eu`: EU sanctions list support (Phase 2)

## Architecture

```
x402-compliance/
├── checker.rs          # Core ComplianceChecker trait + builder
├── lists/
│   ├── ofac.rs         # OFAC SDN list implementation
│   ├── blacklist.rs    # Custom blacklist
│   └── mod.rs          # SanctionsList trait
├── extractors/
│   ├── evm.rs          # EVM address extraction
│   └── solana.rs       # Solana address extraction
├── audit_logger.rs     # Structured compliance logging
├── config.rs           # Configuration management
└── error.rs            # Error types
```

## Compliance Coverage

### Currently Supported (Phase 1)
- ✅ OFAC SDN List (US Treasury)
- ✅ Custom blacklist/allowlist
- ✅ Dual screening (payer + payee)
- ✅ EVM address extraction
- ✅ Solana address extraction
- ✅ Structured audit logging

### Planned (Phase 2)
- UN Consolidated Sanctions List
- UK OFSI Sanctions List
- EU Consolidated Restrictive Measures
- BIS Export Control Lists
- Fuzzy matching algorithms
- 50% Ownership Rule
- Travel Rule (FATF Recommendation 16)

## Testing

```bash
# Run unit tests
cargo test -p x402-compliance

# Run with Solana feature
cargo test -p x402-compliance --features solana

# Run specific test
cargo test -p x402-compliance --test integration_tests
```

## Examples

See `examples/` directory for complete integration examples.

## License

MIT OR Apache-2.0

## Contributing

Contributions welcome! This is a community-driven compliance module designed to benefit all x402 facilitators.

## Security

**IMPORTANT**: This library provides compliance tooling but does not constitute legal advice. Organizations should consult with qualified legal counsel specializing in financial services regulation and sanctions law before relying on this code for production compliance.

Report security issues to: security@ultravioletadao.xyz

## Changelog

### v0.1.0 (2025-11-10)
- Initial release
- OFAC SDN screening
- Custom blacklist support
- EVM + Solana address extractors
- Structured audit logging
- Builder pattern configuration
