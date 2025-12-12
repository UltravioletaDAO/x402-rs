# Stellar/Soroban Integration Plan for x402-rs

**Version**: 1.0
**Date**: December 5, 2025
**Status**: Ready for Implementation
**Target**: Programmatic payments (API/CLI usage)

---

## Executive Summary

This document provides a complete implementation plan for adding Stellar/Soroban support to the x402-rs payment facilitator. The integration leverages Soroban's native authorization framework which provides functionality equivalent to EIP-3009.

**Key Differentiators from EVM**:
- Ledger-based expiration (not timestamps)
- Pre-authorized invocation trees (not raw signatures)
- Two-signature model (client authorizes transfer, facilitator pays fees)
- Mandatory simulation for verification

---

## Table of Contents

1. [Technical Architecture](#1-technical-architecture)
2. [Crate Dependencies](#2-crate-dependencies)
3. [Type Definitions](#3-type-definitions)
4. [Provider Implementation](#4-provider-implementation)
5. [Verification Flow](#5-verification-flow)
6. [Settlement Flow](#6-settlement-flow)
7. [Replay Protection](#7-replay-protection)
8. [Error Handling](#8-error-handling)
9. [Environment Configuration](#9-environment-configuration)
10. [Integration Steps](#10-integration-steps)
11. [Testing Strategy](#11-testing-strategy)
12. [Production Considerations](#12-production-considerations)

---

## 1. Technical Architecture

### Overview

```
                    +-----------------------+
                    |   Client Application  |
                    +-----------+-----------+
                                |
                    1. Create authorization entry
                    2. Sign with user's key
                                |
                                v
                    +-----------+-----------+
                    |  x402-rs Facilitator  |
                    |   (StellarProvider)   |
                    +-----------+-----------+
                                |
                    3. Validate authorization
                    4. Simulate transaction
                    5. Submit with facilitator signature
                                |
                                v
                    +-----------+-----------+
                    |   Soroban RPC Node    |
                    +-----------+-----------+
                                |
                                v
                    +-----------+-----------+
                    |  Stellar Network      |
                    |  (USDC SAC Contract)  |
                    +-----------------------+
```

### Authorization Model

Soroban uses **pre-signed authorization entries** instead of EIP-3009's `transferWithAuthorization`:

```
Authorization Entry Structure:
{
    credentials: {
        address: "G...",          // User's Stellar address
        nonce: u64,               // Unique nonce for replay protection
        signature_expiration_ledger: u32  // Ledger-based expiration
    },
    root_invocation: {
        contract_id: "CCW67...",  // USDC contract
        function_name: "transfer",
        args: [from, to, amount]
    },
    signature: [u8; 64]           // Ed25519 signature
}
```

### USDC Contract Details

| Network | USDC Contract ID | Issuer Address |
|---------|------------------|----------------|
| Mainnet | `CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75` | `GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN` |
| Testnet | `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA` | TBD |

**Token Decimals**: 7 (1 USDC = 10^7 stroops)

---

## 2. Crate Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# Stellar/Soroban support
stellar-sdk = "0.12"           # High-level Stellar SDK
soroban-sdk = "22.0"           # Soroban XDR types and auth
stellar-strkey = "0.0.8"       # Address encoding
ed25519-dalek = "2.1"          # Ed25519 signature verification
sha2 = "0.10"                  # SHA-256 for signature hashing

[features]
default = ["evm", "solana", "near", "stellar"]
stellar = ["stellar-sdk", "soroban-sdk", "stellar-strkey"]
```

### Crate Usage Matrix

| Crate | Purpose |
|-------|---------|
| `stellar-sdk` | RPC client (`Server`), transaction building, account loading |
| `soroban-sdk` | XDR types (`SorobanAuthorizationEntry`, `ScVal`), auth verification |
| `stellar-strkey` | Address validation (`G...` format) |
| `ed25519-dalek` | Signature verification |
| `sha2` | Hash preimage construction for signatures |

---

## 3. Type Definitions

### 3.1 Network Enum Updates (`src/network.rs`)

```rust
// Add to NetworkFamily enum
pub enum NetworkFamily {
    Evm,
    Solana,
    Near,
    Stellar,  // NEW
}

// Add to Network enum
pub enum Network {
    // ... existing variants ...

    #[serde(rename = "stellar")]
    #[display("Stellar Mainnet")]
    Stellar,

    #[serde(rename = "stellar-testnet")]
    #[display("Stellar Testnet")]
    StellarTestnet,
}

// Update From<Network> for NetworkFamily
impl From<Network> for NetworkFamily {
    fn from(value: Network) -> Self {
        match value {
            // ... existing ...
            Network::Stellar | Network::StellarTestnet => NetworkFamily::Stellar,
        }
    }
}
```

### 3.2 USDC Deployment (`src/network.rs`)

```rust
pub const USDC_STELLAR: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";
pub const USDC_STELLAR_TESTNET: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";

impl USDCDeployment {
    pub fn by_network(network: Network) -> Option<Self> {
        match network {
            // ... existing ...
            Network::Stellar => Some(Self {
                address: USDC_STELLAR.to_string(),
                decimals: 7,
            }),
            Network::StellarTestnet => Some(Self {
                address: USDC_STELLAR_TESTNET.to_string(),
                decimals: 7,
            }),
        }
    }
}
```

### 3.3 Payment Payload (`src/types.rs`)

```rust
// Add to ExactPaymentPayload enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "payloadType")]
pub enum ExactPaymentPayload {
    // ... existing variants ...

    #[serde(rename = "stellar")]
    Stellar(ExactStellarPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactStellarPayload {
    /// Stellar account paying (G... address)
    pub from: String,

    /// Stellar account receiving payment (G... address)
    pub to: String,

    /// Payment amount in stroops (1 USDC = 10^7 stroops)
    #[serde(with = "serde_helpers::string_to_u128")]
    pub amount: u128,

    /// Token contract address (e.g., CCW67... for USDC)
    pub token_contract: String,

    /// Pre-authorized invocation signature (base64-encoded XDR)
    pub authorization_entry_xdr: String,

    /// Client-provided nonce (for replay protection)
    pub nonce: u64,

    /// Signature expiration ledger number
    pub signature_expiration_ledger: u32,
}
```

---

## 4. Provider Implementation

### 4.1 StellarProvider Struct (`src/chain/stellar.rs`)

```rust
use stellar_sdk::{Server, Keypair, Network as StellarNetwork};
use soroban_sdk::xdr::SorobanAuthorizationEntry;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct StellarProvider {
    /// Soroban RPC client
    server: Server,

    /// Facilitator's signing keypair
    facilitator_keypair: Keypair,

    /// Network identifier
    stellar_network: StellarNetwork,

    /// Network enum (for logging/metrics)
    network: Network,

    /// Nonce tracker for replay protection
    /// Key: (from_address, nonce), Value: expiration_ledger
    nonce_store: Arc<RwLock<HashMap<(String, u64), u32>>>,

    /// USDC token contract ID
    usdc_contract_id: String,
}
```

### 4.2 Trait Implementations

```rust
// Facilitator trait
impl Facilitator for StellarProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error>;
    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error>;
    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error>;
    fn blacklist_info(&self) -> &'static str;
}

// NetworkProviderOps trait
impl NetworkProviderOps for StellarProvider {
    fn signer_address(&self) -> String {
        self.facilitator_keypair.public_key().to_string()
    }

    fn network(&self) -> Network {
        self.network.clone()
    }
}

// FromEnvByNetworkBuild trait
impl FromEnvByNetworkBuild for StellarProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>>;
}
```

---

## 5. Verification Flow

### 5.1 verify() Implementation

```rust
async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
    let payload = match &request.payment_payload.payload {
        ExactPaymentPayload::Stellar(p) => p,
        _ => return Err(FacilitatorLocalError::PayloadMismatch),
    };

    // Step 1: Decode XDR authorization entry
    let auth_entry = self.decode_authorization_entry(&payload.authorization_entry_xdr)?;

    // Step 2: Validate authorization structure
    self.validate_auth_entry_structure(&auth_entry, payload)?;

    // Step 3: Check signature expiration (ledger-based)
    self.validate_expiration(payload.signature_expiration_ledger).await?;

    // Step 4: Verify cryptographic signature
    self.verify_authorization_signature(&auth_entry, &payload.from)?;

    // Step 5: Check nonce uniqueness (replay protection)
    self.check_nonce_unused(&payload.from, payload.nonce).await?;

    // Step 6: Validate token contract and function call
    self.validate_token_contract(&auth_entry, payload)?;

    // Step 7: Simulate transaction (dry-run)
    self.simulate_transfer(&auth_entry, payload).await?;

    Ok(VerifyResponse::Valid {
        payer: payload.from.clone(),
    })
}
```

### 5.2 Validation Details

**Expiration Validation**:
```rust
async fn validate_expiration(&self, expiry_ledger: u32) -> Result<(), FacilitatorLocalError> {
    let latest_ledger = self.server.get_latest_ledger().await?.sequence;

    if expiry_ledger <= latest_ledger {
        return Err(FacilitatorLocalError::Stellar(StellarError::AuthExpired {
            expiry_ledger,
            current_ledger: latest_ledger,
        }));
    }

    // Warn if expiration too far (>1 hour ~= 720 ledgers)
    if expiry_ledger > latest_ledger + 720 {
        tracing::warn!(
            "Signature expiration unusually far in future: {} ledgers",
            expiry_ledger - latest_ledger
        );
    }

    Ok(())
}
```

**Signature Verification**:
```rust
fn verify_authorization_signature(
    &self,
    auth_entry: &SorobanAuthorizationEntry,
    from: &str,
) -> Result<(), FacilitatorLocalError> {
    // Extract public key from address
    let public_key = stellar_strkey::decode_stellar_address(from)?;

    // Compute signature payload (network-specific hash)
    let signature_payload = self.compute_signature_payload(auth_entry);

    // Extract signature from auth entry
    let signature = self.extract_signature(auth_entry)?;

    // Verify Ed25519 signature
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&public_key)?;
    verifying_key.verify(&signature_payload, &signature)?;

    Ok(())
}
```

**Transaction Simulation**:
```rust
async fn simulate_transfer(
    &self,
    auth_entry: &SorobanAuthorizationEntry,
    payload: &ExactStellarPayload,
) -> Result<SimulationResult, FacilitatorLocalError> {
    // Build transaction envelope
    let tx = self.build_transfer_tx(auth_entry, payload)?;

    // Call simulateTransaction RPC
    let sim_result = self.server.simulate_transaction(tx).await?;

    // Check for errors
    if let Some(error) = sim_result.error {
        return Err(FacilitatorLocalError::Stellar(StellarError::SimulationFailed {
            error,
            diagnostic_events: sim_result.events,
        }));
    }

    Ok(sim_result)
}
```

---

## 6. Settlement Flow

### 6.1 settle() Implementation

```rust
async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
    let payload = match &request.payment_payload.payload {
        ExactPaymentPayload::Stellar(p) => p,
        _ => return Err(FacilitatorLocalError::PayloadMismatch),
    };

    // Step 1: Re-verify (defensive check)
    self.verify(request).await?;

    // Step 2: Build transaction with pre-authorized invocation
    let tx_envelope = self.build_transfer_transaction(payload).await?;

    // Step 3: Sign transaction (facilitator as fee payer)
    let signed_tx = tx_envelope.sign(&self.facilitator_keypair);

    // Step 4: Submit transaction to network
    let response = self.server.send_transaction(signed_tx).await
        .map_err(|e| FacilitatorLocalError::Stellar(StellarError::SubmissionFailed {
            source: e,
        }))?;

    // Step 5: Mark nonce as used
    {
        let mut store = self.nonce_store.write().await;
        store.insert(
            (payload.from.clone(), payload.nonce),
            payload.signature_expiration_ledger,
        );
    }

    // Step 6: Return transaction hash
    Ok(SettleResponse {
        success: true,
        transaction: response.hash,
        network: self.network.clone(),
    })
}
```

### 6.2 Transaction Construction

```rust
async fn build_transfer_transaction(
    &self,
    payload: &ExactStellarPayload,
) -> Result<TransactionEnvelope, FacilitatorLocalError> {
    // Decode authorization entry
    let auth_entry = self.decode_authorization_entry(&payload.authorization_entry_xdr)?;

    // Get facilitator account (for sequence number)
    let facilitator_account = self.server
        .load_account(&self.facilitator_keypair.public_key())
        .await?;

    // Build Soroban invoke contract operation
    let operation = Operation::InvokeContract {
        contract_id: payload.token_contract.clone(),
        function_name: "transfer".to_string(),
        args: vec![
            ScVal::Address(payload.from.parse()?),
            ScVal::Address(payload.to.parse()?),
            ScVal::I128(payload.amount.into()),
        ],
        auth: vec![auth_entry],  // Pre-signed authorization
    };

    // Simulate to get footprint and resource limits
    let sim_result = self.simulate_transfer(&auth_entry, payload).await?;

    // Build transaction with resource limits from simulation
    let tx = Transaction::builder()
        .source_account(facilitator_account)
        .add_operation(operation)
        .set_footprint(sim_result.footprint)
        .set_soroban_data(sim_result.soroban_data)
        .set_max_fee(sim_result.min_resource_fee * 120 / 100)  // 20% buffer
        .build();

    Ok(tx)
}
```

**Key Points**:
- Facilitator is the transaction source (pays fees)
- Client's authorization is embedded in `auth` field
- Resource limits come from simulation
- 20% fee buffer for safety

---

## 7. Replay Protection

### 7.1 Nonce Management

```rust
// Check nonce hasn't been used
async fn check_nonce_unused(&self, from: &str, nonce: u64) -> Result<(), FacilitatorLocalError> {
    let store = self.nonce_store.read().await;

    if store.contains_key(&(from.to_string(), nonce)) {
        return Err(FacilitatorLocalError::Stellar(StellarError::NonceReused {
            from: from.to_string(),
            nonce,
        }));
    }

    Ok(())
}

// Periodic cleanup of expired nonces
async fn cleanup_expired_nonces(&self, current_ledger: u32) {
    let mut store = self.nonce_store.write().await;
    store.retain(|_, expiry_ledger| *expiry_ledger > current_ledger);
}
```

### 7.2 Production Nonce Storage (Redis)

For production, migrate to Redis with TTL:

```rust
async fn mark_nonce_used_redis(
    redis: &RedisClient,
    from: &str,
    nonce: u64,
    expiry_ledger: u32,
    current_ledger: u32,
) -> Result<()> {
    let key = format!("stellar:nonce:{}:{}", from, nonce);
    let ttl_seconds = (expiry_ledger.saturating_sub(current_ledger)) * 5;  // 5 sec/ledger
    redis.set_ex(key, "1", ttl_seconds).await?;
    Ok(())
}
```

---

## 8. Error Handling

### 8.1 Stellar-Specific Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum StellarError {
    #[error("Invalid XDR encoding: {0}")]
    InvalidXdr(String),

    #[error("Authorization expired at ledger {expiry_ledger} (current: {current_ledger})")]
    AuthExpired {
        expiry_ledger: u32,
        current_ledger: u32,
    },

    #[error("Invalid authorization signature for address {address}")]
    InvalidSignature { address: String },

    #[error("Nonce {nonce} already used for address {from}")]
    NonceReused { from: String, nonce: u64 },

    #[error("Simulation failed: {error}")]
    SimulationFailed {
        error: String,
        diagnostic_events: Vec<DiagnosticEvent>,
    },

    #[error("Transaction submission failed")]
    SubmissionFailed { source: stellar_sdk::Error },

    #[error("Token contract mismatch: expected {expected}, got {actual}")]
    TokenContractMismatch { expected: String, actual: String },

    #[error("Insufficient balance: {from} has {balance}, needs {amount}")]
    InsufficientBalance {
        from: String,
        balance: u128,
        amount: u128,
    },

    #[error("RPC error: {0}")]
    RpcError(String),
}
```

### 8.2 HTTP Status Code Mapping

| Error | HTTP Status |
|-------|-------------|
| `InvalidXdr` | 400 Bad Request |
| `AuthExpired` | 400 Bad Request |
| `InvalidSignature` | 400 Bad Request |
| `NonceReused` | 409 Conflict |
| `SimulationFailed` | 400 Bad Request |
| `SubmissionFailed` | 500 Internal Server Error |
| `InsufficientBalance` | 400 Bad Request |
| `RpcError` | 503 Service Unavailable |

---

## 9. Environment Configuration

### 9.1 New Environment Variables

```bash
# .env.example additions

# Stellar Mainnet
RPC_URL_STELLAR_MAINNET=https://soroban-mainnet.stellar.org
STELLAR_PRIVATE_KEY_MAINNET=S...  # Facilitator secret key

# Stellar Testnet
RPC_URL_STELLAR_TESTNET=https://soroban-testnet.stellar.org
STELLAR_PRIVATE_KEY_TESTNET=S...  # Testnet facilitator secret key
```

### 9.2 Environment Loading (`src/from_env.rs`)

```rust
pub const RPC_URL_STELLAR_MAINNET: &str = "RPC_URL_STELLAR_MAINNET";
pub const RPC_URL_STELLAR_TESTNET: &str = "RPC_URL_STELLAR_TESTNET";
pub const STELLAR_PRIVATE_KEY_MAINNET: &str = "STELLAR_PRIVATE_KEY_MAINNET";
pub const STELLAR_PRIVATE_KEY_TESTNET: &str = "STELLAR_PRIVATE_KEY_TESTNET";

pub fn rpc_env_name_from_network(network: Network) -> Option<&'static str> {
    match network {
        // ... existing ...
        Network::Stellar => Some(RPC_URL_STELLAR_MAINNET),
        Network::StellarTestnet => Some(RPC_URL_STELLAR_TESTNET),
    }
}

pub fn stellar_private_key_env_name(network: Network) -> Option<&'static str> {
    match network {
        Network::Stellar => Some(STELLAR_PRIVATE_KEY_MAINNET),
        Network::StellarTestnet => Some(STELLAR_PRIVATE_KEY_TESTNET),
        _ => None,
    }
}
```

---

## 10. Integration Steps

### Phase 1: Core Backend (Week 1)

```
Day 1-2: Type Definitions
[ ] Add NetworkFamily::Stellar to src/network.rs
[ ] Add Network::Stellar and Network::StellarTestnet
[ ] Add USDC_STELLAR constants
[ ] Add ExactStellarPayload to src/types.rs
[ ] Update USDCDeployment::by_network()

Day 3-4: Provider Skeleton
[ ] Create src/chain/stellar.rs
[ ] Define StellarProvider struct
[ ] Implement FromEnvByNetworkBuild
[ ] Add to NetworkProvider enum in src/chain/mod.rs
[ ] Update dispatch logic in NetworkProvider::verify/settle

Day 5: XDR Handling
[ ] Implement decode_authorization_entry()
[ ] Implement compute_signature_payload()
[ ] Implement verify_authorization_signature()
[ ] Add unit tests for XDR parsing
```

### Phase 2: Verification & Settlement (Week 2)

```
Day 1-2: Verification
[ ] Implement StellarProvider::verify()
[ ] Implement validate_expiration()
[ ] Implement check_nonce_unused()
[ ] Implement simulate_transfer()
[ ] Add unit tests for verification

Day 3-4: Settlement
[ ] Implement StellarProvider::settle()
[ ] Implement build_transfer_transaction()
[ ] Implement nonce marking logic
[ ] Add unit tests for settlement

Day 5: Error Handling
[ ] Define StellarError enum
[ ] Implement From<StellarError> for FacilitatorLocalError
[ ] Add HTTP status code mappings
```

### Phase 3: Testing & Integration (Week 3)

```
Day 1-2: Integration Tests
[ ] Create tests/integration/test_stellar_payment.py
[ ] Test verify() with valid authorization
[ ] Test verify() with expired authorization
[ ] Test verify() with invalid signature
[ ] Test settle() end-to-end on testnet

Day 3: Frontend Integration
[ ] Add stellar.png logo to static/images/
[ ] Add logo handler to src/handlers.rs
[ ] Add Stellar network cards to static/index.html

Day 4: Environment & Deployment
[ ] Add RPC URLs to AWS Secrets Manager
[ ] Fund testnet facilitator wallet
[ ] Update .env.example
[ ] Update README.md

Day 5: Documentation
[ ] Create guides/STELLAR_CLIENT_GUIDE.md
[ ] Update ADDING_NEW_CHAINS.md with Stellar example
```

### Phase 4: Production Hardening (Week 4)

```
[ ] Migrate nonce store to Redis
[ ] Add CloudWatch metrics (latency, error rates)
[ ] Add facilitator XLM balance monitoring
[ ] Add alerting for low balance (<10 XLM)
[ ] Security review
[ ] Load testing against testnet
[ ] Deploy to mainnet
```

---

## 11. Testing Strategy

### 11.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_authorization_entry() {
        // Test XDR decoding with known test vectors
    }

    #[test]
    fn test_verify_signature_valid() {
        // Test signature verification with known good signature
    }

    #[test]
    fn test_verify_signature_invalid() {
        // Test signature verification with bad signature
    }

    #[test]
    fn test_nonce_uniqueness() {
        // Test nonce tracking and rejection
    }

    #[test]
    fn test_expiration_validation() {
        // Test ledger-based expiration checks
    }
}
```

### 11.2 Integration Tests

```python
# tests/integration/test_stellar_payment.py

def test_stellar_verify_valid():
    """Test verification of valid Stellar authorization."""
    payload = create_test_stellar_payload()
    response = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert response.status_code == 200
    assert response.json()["valid"] == True

def test_stellar_verify_expired():
    """Test rejection of expired authorization."""
    payload = create_expired_stellar_payload()
    response = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert response.status_code == 400
    assert "expired" in response.json()["error"].lower()

def test_stellar_settle_success():
    """Test successful settlement on testnet."""
    payload = create_test_stellar_payload()
    response = requests.post(f"{FACILITATOR_URL}/settle", json=payload)
    assert response.status_code == 200
    assert response.json()["success"] == True
    assert response.json()["transaction"] is not None

def test_stellar_replay_protection():
    """Test rejection of duplicate nonce."""
    payload = create_test_stellar_payload()
    # First submission succeeds
    response1 = requests.post(f"{FACILITATOR_URL}/settle", json=payload)
    assert response1.status_code == 200
    # Second submission fails
    response2 = requests.post(f"{FACILITATOR_URL}/settle", json=payload)
    assert response2.status_code == 409
```

### 11.3 Load Testing

```bash
# k6 load test
k6 run --vus 10 --duration 5m tests/load/stellar_load_test.js
```

Target metrics:
- p99 latency: <5 seconds
- Error rate: <0.1%
- Throughput: >10 TPS

---

## 12. Production Considerations

### 12.1 Facilitator Wallet Management

**XLM Balance Requirements**:
- Minimum reserve: 10 XLM
- Recommended: 100 XLM
- Average fee per transaction: ~0.0001 XLM

**Monitoring**:
```rust
async fn check_facilitator_balance(server: &Server, keypair: &Keypair) -> Result<u64> {
    let account = server.load_account(&keypair.public_key()).await?;
    let xlm_balance = account.native_balance()?;

    if xlm_balance < 10_000_000 {  // 10 XLM in stroops
        tracing::warn!("Facilitator XLM balance low: {} stroops", xlm_balance);
    }

    Ok(xlm_balance)
}
```

### 12.2 Observability

**Metrics**:
- `stellar_verify_latency_ms` - Verification time
- `stellar_settle_latency_ms` - Settlement time
- `stellar_nonce_store_size` - Cache size
- `stellar_expired_authorizations_total` - Expired rejections
- `stellar_simulation_failures_total` - Simulation errors

**Logs**:
```rust
tracing::info!(
    network = %self.network,
    from = %payload.from,
    to = %payload.to,
    amount = %payload.amount,
    tx_hash = %tx_hash,
    "Stellar payment settled"
);
```

### 12.3 Security Hardening

**Input Validation**:
- Reject XDR payloads > 10KB
- Validate G... address format
- Enforce minimum amount (e.g., 0.01 USDC)
- Enforce maximum amount (e.g., 10,000 USDC)

**Rate Limiting**:
- Per-address: 10 settle requests/minute
- Global: 100 settle requests/minute

### 12.4 RPC Provider Selection

| Provider | Rate Limit | Recommended For |
|----------|------------|-----------------|
| soroban.stellar.org (free) | ~10 req/s | Development/Testing |
| QuickNode | Higher | Production |
| Blockdaemon | Higher | Production |
| Self-hosted | Unlimited | High-volume production |

---

## Appendix A: Client Integration Example

### JavaScript/TypeScript Client

```typescript
import { Server, Keypair, TransactionBuilder, Operation } from '@stellar/stellar-sdk';

async function createStellarPayment(
    from: string,
    to: string,
    amount: string,
    facilitatorUrl: string
): Promise<PaymentResult> {
    const server = new Server('https://soroban-mainnet.stellar.org');

    // Get current ledger
    const ledgerInfo = await server.getLatestLedger();
    const expirationLedger = ledgerInfo.sequence + 720;  // ~1 hour

    // Generate nonce
    const nonce = Date.now();  // Simple nonce strategy

    // Build authorization entry
    const authEntry = buildTransferAuthEntry({
        from,
        to,
        amount,
        tokenContract: USDC_CONTRACT,
        nonce,
        expirationLedger,
    });

    // Sign authorization entry with user's key
    const signedAuthEntry = signAuthEntry(authEntry, userKeypair);

    // Encode to XDR base64
    const authEntryXdr = signedAuthEntry.toXDR('base64');

    // Submit to facilitator
    const response = await fetch(`${facilitatorUrl}/settle`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            x402Version: 'v1',
            scheme: 'exact',
            network: 'stellar',
            payload: {
                payloadType: 'stellar',
                from,
                to,
                amount,
                tokenContract: USDC_CONTRACT,
                authorizationEntryXdr: authEntryXdr,
                nonce,
                signatureExpirationLedger: expirationLedger,
            },
        }),
    });

    return response.json();
}
```

### Rust Client

```rust
use stellar_sdk::{Server, Keypair};
use reqwest::Client;

async fn create_stellar_payment(
    from: &str,
    to: &str,
    amount: u128,
    user_keypair: &Keypair,
    facilitator_url: &str,
) -> Result<PaymentResult, Error> {
    let server = Server::new("https://soroban-mainnet.stellar.org")?;

    // Get current ledger
    let ledger_info = server.get_latest_ledger().await?;
    let expiration_ledger = ledger_info.sequence + 720;

    // Generate nonce
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;

    // Build and sign authorization entry
    let auth_entry = build_transfer_auth_entry(
        from, to, amount, USDC_CONTRACT, nonce, expiration_ledger
    )?;
    let signed_auth = sign_auth_entry(&auth_entry, user_keypair)?;
    let auth_entry_xdr = signed_auth.to_xdr_base64()?;

    // Submit to facilitator
    let client = Client::new();
    let response = client
        .post(&format!("{}/settle", facilitator_url))
        .json(&serde_json::json!({
            "x402Version": "v1",
            "scheme": "exact",
            "network": "stellar",
            "payload": {
                "payloadType": "stellar",
                "from": from,
                "to": to,
                "amount": amount.to_string(),
                "tokenContract": USDC_CONTRACT,
                "authorizationEntryXdr": auth_entry_xdr,
                "nonce": nonce,
                "signatureExpirationLedger": expiration_ledger,
            }
        }))
        .send()
        .await?;

    response.json().await
}
```

---

## Appendix B: Network Passphrase Constants

```rust
pub const STELLAR_MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
pub const STELLAR_TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

impl Network {
    pub fn stellar_passphrase(&self) -> Option<&'static str> {
        match self {
            Network::Stellar => Some(STELLAR_MAINNET_PASSPHRASE),
            Network::StellarTestnet => Some(STELLAR_TESTNET_PASSPHRASE),
            _ => None,
        }
    }
}
```

---

## Summary

This implementation plan provides a complete roadmap for adding Stellar/Soroban support to x402-rs. Key highlights:

1. **Authorization Model**: Uses Soroban's native `require_auth` with pre-signed authorization entries
2. **Replay Protection**: Nonce tracking with ledger-based expiration
3. **Fee Abstraction**: Facilitator pays all transaction fees
4. **Clean Architecture**: Follows existing provider pattern

**Estimated Total Effort**: 3-4 weeks for production-ready implementation

**Next Steps**:
1. Review plan with team
2. Set up Stellar testnet environment
3. Begin Phase 1 implementation
