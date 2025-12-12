# x402-rs Architecture Quick Reference

**Version**: 1.7.7 | **Purpose**: Fast lookup for common patterns and locations

---

## 🗺️ Critical File Locations

| Purpose | File Path | Lines |
|---------|-----------|-------|
| HTTP server entrypoint | `src/main.rs` | 137 |
| Protocol types | `src/types.rs` | 1537 |
| Network definitions | `src/network.rs` | 790 |
| Core trait | `src/facilitator.rs` | 114 |
| Main implementation | `src/facilitator_local.rs` | 370 |
| HTTP handlers | `src/handlers.rs` | 404 |
| EVM logic | `src/chain/evm.rs` | ~1800 |
| Solana/Fogo logic | `src/chain/solana.rs` | ~800 |
| NEAR logic | `src/chain/near.rs` | ~700 |
| Stellar logic | `src/chain/stellar.rs` | ~900 |
| Provider cache | `src/provider_cache.rs` | 93 |
| Compliance module | `crates/x402-compliance/` | workspace |

---

## 🔄 Data Flow Patterns

### Payment Verification Flow
```
POST /verify → handlers::post_verify()
  → FacilitatorLocal::verify()
    → [Compliance Screening]
      → ProviderMap::by_network()
        → NetworkProvider::verify() [enum dispatch]
          → EvmProvider::verify() | SolanaProvider::verify() | etc.
            → [Chain-specific validation]
              → VerifyResponse::Valid{payer} | Invalid{reason}
```

### Payment Settlement Flow
```
POST /settle → handlers::post_settle()
  → FacilitatorLocal::settle()
    → [Compliance RE-SCREENING] ⚠️ CRITICAL
      → ProviderMap::by_network()
        → NetworkProvider::settle() [enum dispatch]
          → EvmProvider::settle() | SolanaProvider::settle() | etc.
            → [On-chain transaction]
              → SettleResponse{success, tx_hash, payer}
```

---

## 🧩 Type Hierarchy

```
PaymentPayload
  ├─ x402_version: X402Version (V1)
  ├─ scheme: Scheme (Exact)
  ├─ network: Network (Base, Avalanche, Solana, NEAR, Stellar, ...)
  └─ payload: ExactPaymentPayload
       ├─ Evm(ExactEvmPayload)
       │    ├─ signature: EvmSignature (65+ bytes, EIP-6492 wrapper possible)
       │    └─ authorization: ExactEvmPayloadAuthorization
       │         ├─ from: EvmAddress
       │         ├─ to: EvmAddress
       │         ├─ value: TokenAmount (U256)
       │         ├─ valid_after: UnixTimestamp
       │         ├─ valid_before: UnixTimestamp
       │         └─ nonce: HexEncodedNonce (32 bytes)
       ├─ Solana(ExactSolanaPayload)
       │    └─ transaction: String (base64-encoded SPL tx)
       ├─ Near(ExactNearPayload)
       │    └─ signed_delegate_action: String (base64-encoded borsh)
       └─ Stellar(ExactStellarPayload)
            ├─ from: String (G... address)
            ├─ to: String (G... address)
            ├─ amount: String (stroops)
            ├─ token_contract: String (C... address)
            ├─ authorization_entry_xdr: String (base64 XDR)
            ├─ nonce: u64
            └─ signature_expiration_ledger: u32
```

---

## 🔧 Common Code Patterns

### Adding a New Network (Checklist)

**1. Update `src/network.rs`:**
```rust
pub enum Network {
    // ... existing variants
    #[serde(rename = "new-network")]
    NewNetwork,
    #[serde(rename = "new-network-testnet")]
    NewNetworkTestnet,
}

// Add to Display impl
Network::NewNetwork => write!(f, "new-network"),

// Add to variants()
Network::NewNetwork,
Network::NewNetworkTestnet,

// Add USDC deployment
static USDC_NEW_NETWORK: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x...").into(),
            network: Network::NewNetwork,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});
```

**2. Update `src/chain/evm.rs` (if EVM-compatible):**
```rust
impl TryFrom<Network> for EvmChain {
    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            // ... existing mappings
            Network::NewNetwork => Ok(EvmChain::new(value, CHAIN_ID)),
        }
    }
}
```

**3. Add RPC configuration (`src/from_env.rs`):**
```rust
Network::NewNetwork => env::var("RPC_URL_NEW_NETWORK").ok(),
```

**4. Update frontend (`static/index.html`):**
- Add network logo handler in `src/handlers.rs`
- Add network card in HTML
- Add CSS styling

**Total**: ~155 lines of code + 1 logo file

### Implementing Facilitator Trait

```rust
impl Facilitator for MyProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        // 1. Parse payload
        let payload = match &request.payment_payload.payload {
            ExactPaymentPayload::Evm(p) => p,
            _ => return Err(FacilitatorLocalError::InvalidScheme),
        };

        // 2. Validate timing
        let now = UnixTimestamp::now()?;
        if now < payload.authorization.valid_after || now >= payload.authorization.valid_before {
            return Err(FacilitatorLocalError::InvalidTiming(
                payload.authorization.from.into(),
                format!("now={}, valid_after={}, valid_before={}",
                    now, payload.authorization.valid_after, payload.authorization.valid_before)
            ));
        }

        // 3. Check receiver matches requirements
        if payload.authorization.to != request.payment_requirements.pay_to.try_into()? {
            return Err(FacilitatorLocalError::ReceiverMismatch(
                payload.authorization.from.into(),
                payload.authorization.to.to_string(),
                request.payment_requirements.pay_to.to_string(),
            ));
        }

        // 4. Verify signature (chain-specific)
        // 5. Check balance (chain-specific)

        Ok(VerifyResponse::Valid {
            payer: payload.authorization.from.into(),
        })
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        // 1. Re-verify (CRITICAL: never trust prior verify call)
        let verify_result = self.verify(request).await?;
        let payer = match verify_result {
            VerifyResponse::Valid { payer } => payer,
            VerifyResponse::Invalid { reason, payer } => {
                return Ok(SettleResponse {
                    success: false,
                    error_reason: Some(reason),
                    payer: payer.unwrap_or_else(|| /* default */),
                    transaction: None,
                    network: request.network(),
                });
            }
        };

        // 2. Execute on-chain transaction (chain-specific)
        // 3. Wait for receipt
        // 4. Return response

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer,
            transaction: Some(tx_hash),
            network: request.network(),
        })
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        Ok(SupportedPaymentKindsResponse {
            kinds: vec![SupportedPaymentKind {
                x402_version: X402Version::V1,
                scheme: Scheme::Exact,
                network: self.network.to_string(),
                extra: Some(SupportedPaymentKindExtra {
                    fee_payer: self.signer_address(),
                }),
            }],
        })
    }
}
```

### Compliance Screening Pattern

```rust
use x402_compliance::{ComplianceChecker, EvmExtractor, ScreeningDecision};

// In verify() or settle():
let (payer, payee) = EvmExtractor::extract_addresses(
    &payload.authorization.from,
    &payload.authorization.to,
)?;

let context = TransactionContext {
    amount: payload.authorization.value.to_string(),
    currency: "USDC".to_string(),
    network: format!("{:?}", network),
    transaction_id: None,
};

let screening_result = compliance_checker
    .screen_payment(&payer, &payee, &context)
    .await?;

match screening_result.decision {
    ScreeningDecision::Block { reason } => {
        return Err(FacilitatorLocalError::BlockedAddress(
            MixedAddress::Evm(payload.authorization.from),
            reason,
        ));
    }
    ScreeningDecision::Review { reason } => {
        return Err(FacilitatorLocalError::BlockedAddress(
            MixedAddress::Evm(payload.authorization.from),
            format!("Manual review required: {}", reason),
        ));
    }
    ScreeningDecision::Clear => {
        // Proceed with payment
    }
}
```

---

## ⚠️ Critical Security Patterns

### 1. Always Re-Verify on Settlement
```rust
// ❌ WRONG: Trusting prior verify call
async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
    // Directly execute transaction without re-verification
    self.execute_transfer(request).await
}

// ✅ CORRECT: Re-verify before settlement
async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
    // Re-verify payment (critical security measure)
    let verify_result = self.verify(request).await?;
    // ... then execute
}
```

### 2. Always Re-Screen Compliance
```rust
// In FacilitatorLocal::settle():
// CRITICAL: Re-screen compliance before settlement (don't trust prior verify call)
self.perform_compliance_screening(&request.payment_payload.payload, network).await?;
```

### 3. Separate Mainnet/Testnet Wallets
```rust
// ✅ CORRECT: Separate wallets per environment
EVM_PRIVATE_KEY_MAINNET=0x...
EVM_PRIVATE_KEY_TESTNET=0x...

// ❌ WRONG: Single wallet for all environments
EVM_PRIVATE_KEY=0x...  // Deprecated pattern
```

### 4. Fail-Closed on Missing Config
```rust
// ✅ CORRECT: Exit on missing critical config
let provider_cache = ProviderCache::from_env().await?;
let provider_cache = match provider_cache {
    Ok(cache) => cache,
    Err(e) => {
        tracing::error!("Failed to create providers: {}", e);
        std::process::exit(1);  // Fail-closed
    }
};

// ❌ WRONG: Continue with degraded service
let provider_cache = ProviderCache::from_env().await.unwrap_or_default();
```

### 5. Never Log Secrets
```rust
// ✅ CORRECT: Redact sensitive data
tracing::debug!("Loaded wallet: {}", wallet.address());

// ❌ WRONG: Logging private keys
tracing::debug!("Private key: {}", private_key);
```

---

## 🔍 Debugging Checklist

### Payment Verification Failures

**1. Check payload structure:**
```bash
# Decode base64 payment payload
echo "<base64_payload>" | base64 -d | jq
```

**2. Verify timestamp format:**
```rust
// ✅ CORRECT: Unix seconds
valid_after: 1702000000
valid_before: 1702003600

// ❌ WRONG: Milliseconds
valid_after: 1702000000000
```

**3. Check network enum match:**
```rust
// ✅ CORRECT: Exact string match
"network": "base-sepolia"

// ❌ WRONG: Case mismatch or wrong format
"network": "Base-Sepolia"  // Capital B
"network": "base_sepolia"  // Underscore instead of hyphen
```

**4. Verify EIP-712 domain separator:**
```bash
# Run domain separator comparison script
python scripts/compare_domain_separator.py --network base-mainnet
```

**5. Enable debug logging:**
```bash
RUST_LOG=debug cargo run --release
```

### Payment Settlement Failures

**1. Check facilitator wallet balance:**
```bash
python scripts/check_config.py
```

**2. Verify RPC connectivity:**
```bash
curl -X POST <RPC_URL> -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

**3. Check nonce tracking:**
```bash
# Look for nonce errors in logs
grep -i "nonce" /var/log/facilitator.log
```

**4. Validate compliance screening:**
```bash
# Check compliance logs
grep -i "compliance\|blocked\|review" /var/log/facilitator.log
```

---

## 📊 Performance Patterns

### Nonce Parallelism (EVM)
```rust
// Multiple wallets with round-robin selection
let signer_addresses = Arc::new(vec![
    wallet1.address(),
    wallet2.address(),
    wallet3.address(),
]);

// Atomic round-robin counter
let index = signer_cursor.fetch_add(1, Ordering::Relaxed) % signer_addresses.len();
let signer_address = signer_addresses[index];
```

**Benefit**: 3 wallets = 3x transaction throughput (parallel nonce sequences)

### Provider Caching
```rust
// ✅ CORRECT: Initialize once, reuse across requests
let provider_cache = ProviderCache::from_env().await?;
let axum_state = Arc::new(FacilitatorLocal::new(provider_cache, compliance_checker));

// ❌ WRONG: Re-initialize per request
async fn handle_request() {
    let provider_cache = ProviderCache::from_env().await?;  // Expensive!
}
```

### Lazy Evaluation
```rust
// ✅ CORRECT: Lazy initialization for constants
static USDC_BASE: Lazy<USDCDeployment> = Lazy::new(|| { /* ... */ });

// ❌ WRONG: Eager initialization in function
fn get_usdc_base() -> USDCDeployment {
    USDCDeployment(TokenDeployment { /* ... */ })  // Allocated every call
}
```

---

## 🌐 Multi-Chain Quick Ref

| Chain Family | Networks | Payment Method | Signature | Decimals |
|--------------|----------|----------------|-----------|----------|
| EVM | Base, Avalanche, Polygon, Optimism, Celo, Ethereum, Arbitrum, etc. (23 total) | EIP-3009 `transferWithAuthorization` | EIP-712 typed data | 6 |
| Solana | Solana mainnet/devnet, Fogo mainnet/testnet | SPL token transfer | Ed25519 | 6 |
| NEAR | NEAR mainnet/testnet | NEP-366 meta-transactions | Ed25519 (delegate action) | 6 |
| Stellar | Stellar mainnet/testnet | Soroban authorization entries | Ed25519 (XDR-encoded) | **7** |

### Chain-Specific Notes

**EVM**:
- Universal validator at `0xdAcD51A54883eb67D95FAEb2BBfdC4a9a6BD2a3B`
- Supports EIP-6492 counterfactual wallets
- Gas pricing: EIP-1559 vs legacy (per network)

**Solana/Fogo**:
- Configurable compute budget via env vars
- Fogo uses SVM (Solana Virtual Machine)
- Facilitator wraps user's signed transaction

**NEAR**:
- Auto-registration: `storage_deposit` (0.00125 NEAR) if recipient not registered
- Uses `near-primitives` 0.34+ API (NearToken, Gas types)
- Implicit USDC account: 64 hex chars (SHA256 hash)

**Stellar**:
- **USDC has 7 decimals** (not 6!)
- Addresses: G... (accounts), C... (contracts)
- Ledger-based expiry (not Unix timestamp)
- Network passphrases: "Public Global Stellar Network ; September 2015" (mainnet)

---

## 📦 Workspace Crates

| Crate | Purpose | Key Exports |
|-------|---------|-------------|
| `x402-rs` (root) | Main facilitator service | Binary executable |
| `crates/x402-compliance` | Modular sanctions screening | `ComplianceChecker`, `OfacChecker`, `ScreeningDecision` |
| `crates/x402-axum` | Axum middleware | `X402Layer`, `PaymentGate` |
| `crates/x402-reqwest` | Client library | `X402Client`, `PaymentBuilder` |

---

## 🚀 Deployment Quick Start

### Local Development
```bash
# 1. Copy environment template
cp .env.example .env

# 2. Configure testnet wallets and RPC URLs
vim .env

# 3. Run facilitator
cargo run --release

# 4. Test health endpoint
curl http://localhost:8080/health
```

### Docker Build
```bash
# Build and push to ECR
./scripts/build-and-push.sh v1.7.7
```

### AWS ECS Deployment
```bash
# Initialize Terraform (first time only)
cd terraform/environments/production
terraform init

# Plan deployment
terraform plan -out=facilitator-prod.tfplan

# Apply changes
terraform apply facilitator-prod.tfplan

# Force new deployment
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

---

## 📚 Key Documentation

| Document | Purpose |
|----------|---------|
| `CLAUDE.md` | Project instructions for AI agents |
| `docs/ARCHITECTURE_SUMMARY_FOR_AGENTS.md` | Comprehensive architecture guide (this file's sibling) |
| `docs/CUSTOMIZATIONS.md` | Fork-specific customizations vs upstream |
| `docs/CHANGELOG.md` | Version history and release notes |
| `guides/ADDING_NEW_CHAINS.md` | Complete checklist for chain integration |
| `docs/COMPLIANCE_INTEGRATION_COMPLETE.md` | Compliance system overview |
| `docs/EIP3009_TIMESTAMP_BEST_PRACTICES.md` | Timestamp handling for EIP-3009 |

---

**Last Updated**: 2025-12-11 | **Document Version**: 1.0
