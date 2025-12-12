# Algorand Integration Plan for x402-rs

**Version**: 1.0
**Date**: December 5, 2025
**Status**: Ready for Implementation
**Target**: Programmatic payments (API/CLI usage)

---

## Executive Summary

This document provides a complete implementation plan for adding Algorand support to the x402-rs payment facilitator. The integration leverages Algorand's **Atomic Transfers** mechanism to achieve gasless, authorized payments similar to EIP-3009.

**Key Differentiators from EVM**:
- Round-based validity windows (not timestamps)
- Atomic Transfer groups (not single-transaction authorization)
- Pre-signed ASA transfers (client signs first, facilitator completes group)
- Two-transaction model: client's ASA transfer + facilitator's fee payment
- No native delegation mechanism - uses grouped pre-signed transactions

**Important Note**: Unlike Stellar's built-in authorization framework, Algorand requires a custom two-stage signing protocol using atomic transfer groups.

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
                    1. Build unsigned ASA transfer
                    2. Send to facilitator for group ID
                                |
                                v
                    +-----------+-----------+
                    |  x402-rs Facilitator  |
                    |  (AlgorandProvider)   |
                    +-----------+-----------+
                                |
                    3. Build atomic group (client tx + fee tx)
                    4. Return group ID to client
                                |
                                v
                    +-----------+-----------+
                    |   Client Application  |
                    +-----------+-----------+
                                |
                    5. Sign ASA transfer with group ID
                    6. Return signed tx to facilitator
                                |
                                v
                    +-----------+-----------+
                    |  x402-rs Facilitator  |
                    +-----------+-----------+
                                |
                    7. Verify signature and group
                    8. Sign fee payment transaction
                    9. Submit atomic group
                                |
                                v
                    +-----------+-----------+
                    |   Algorand Node       |
                    |   (algod/indexer)     |
                    +-----------------------+
```

### Authorization Model: Atomic Transfers

Algorand uses **Atomic Transfers** for grouped transaction execution:

```
Atomic Transfer Group Structure:
{
    group_id: [32 bytes],  // SHA-512/256 hash of all tx IDs
    transactions: [
        {
            // Transaction 0: Client's ASA Transfer
            type: "axfer",
            sender: "CLIENT_ADDRESS",     // Client pays USDC
            receiver: "MERCHANT_ADDRESS",
            amount: 1000000,              // 1 USDC (6 decimals)
            asset_id: 31566704,           // USDC ASA ID
            first_valid: 12345678,        // Round-based validity
            last_valid: 12345778,         // 100-round window
            signature: [64 bytes]         // Client's Ed25519 signature
        },
        {
            // Transaction 1: Facilitator's Fee Payment
            type: "pay",
            sender: "FACILITATOR_ADDRESS",
            receiver: "CLIENT_ADDRESS",   // Cover client's min balance
            amount: 0,                    // Or small fee rebate
            first_valid: 12345678,
            last_valid: 12345778,
            signature: [64 bytes]         // Facilitator's signature
        }
    ]
}
```

**Key Properties**:
- All transactions in group succeed or fail together
- Group ID computed BEFORE signing (ties transactions together)
- Client signs only their ASA transfer
- Facilitator signs only their fee payment
- Neither party can modify the other's transaction

### USDC ASA Details

| Network | USDC ASA ID | Creator Address |
|---------|-------------|-----------------|
| Mainnet | `31566704` | Circle (verified) |
| Testnet | `10458941` | TestNet USDC (dispensable) |

**Token Decimals**: 6 (1 USDC = 1,000,000 micro-USDC)

---

## 2. Crate Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# Algorand support
algonaut = "0.4"              # Algorand SDK (algod + indexer clients)
algonaut-core = "0.4"         # Core types (Transaction, Address)
algonaut-crypto = "0.4"       # Ed25519 signing, hashing
algonaut-encoding = "0.4"     # MessagePack encoding
ed25519-dalek = "2.1"         # Signature verification
sha2 = "0.10"                 # SHA-512/256 for group ID

[features]
default = ["evm", "solana", "near", "stellar", "algorand"]
algorand = ["algonaut", "algonaut-core", "algonaut-crypto", "algonaut-encoding"]
```

### Crate Usage Matrix

| Crate | Purpose |
|-------|---------|
| `algonaut` | High-level client (`Algod`, `Indexer`), transaction building |
| `algonaut-core` | `Transaction`, `Address`, `AssetTransferTransaction` types |
| `algonaut-crypto` | Ed25519 verification, signature encoding |
| `algonaut-encoding` | MessagePack (msgpack) transaction serialization |
| `sha2` | SHA-512/256 for computing group ID |

---

## 3. Type Definitions

### 3.1 Network Enum Updates (`src/network.rs`)

```rust
// Add to NetworkFamily enum
pub enum NetworkFamily {
    Evm,
    Solana,
    Near,
    Stellar,
    Algorand,  // NEW
}

// Add to Network enum
pub enum Network {
    // ... existing variants ...

    #[serde(rename = "algorand")]
    #[display("Algorand Mainnet")]
    Algorand,

    #[serde(rename = "algorand-testnet")]
    #[display("Algorand Testnet")]
    AlgorandTestnet,
}

// Update From<Network> for NetworkFamily
impl From<Network> for NetworkFamily {
    fn from(value: Network) -> Self {
        match value {
            // ... existing ...
            Network::Algorand | Network::AlgorandTestnet => NetworkFamily::Algorand,
        }
    }
}
```

### 3.2 USDC Deployment (`src/network.rs`)

```rust
pub const USDC_ALGORAND_MAINNET: u64 = 31566704;
pub const USDC_ALGORAND_TESTNET: u64 = 10458941;

impl USDCDeployment {
    pub fn by_network(network: Network) -> Option<Self> {
        match network {
            // ... existing ...
            Network::Algorand => Some(Self {
                address: USDC_ALGORAND_MAINNET.to_string(),
                decimals: 6,
            }),
            Network::AlgorandTestnet => Some(Self {
                address: USDC_ALGORAND_TESTNET.to_string(),
                decimals: 6,
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

    #[serde(rename = "algorand")]
    Algorand(ExactAlgorandPayload),
}

/// Algorand payment payload for atomic transfer authorization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactAlgorandPayload {
    /// Algorand account paying (58-char base32 address)
    pub from: String,

    /// Algorand account receiving payment
    pub to: String,

    /// Payment amount in micro-USDC (1 USDC = 1,000,000)
    #[serde(with = "serde_helpers::string_to_u64")]
    pub amount: u64,

    /// USDC ASA ID (31566704 for mainnet)
    pub asset_id: u64,

    /// Pre-signed ASA transfer transaction (base64-encoded msgpack)
    pub signed_transaction: String,

    /// Transaction ID of the ASA transfer (for verification)
    pub tx_id: String,

    /// First valid round for the transaction
    pub first_valid: u64,

    /// Last valid round for the transaction
    pub last_valid: u64,

    /// Atomic group ID (base64-encoded, 32 bytes)
    pub group_id: String,
}
```

### 3.4 Intermediate Types for Two-Stage Protocol

```rust
/// Request for group ID generation (Stage 1)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlgorandPrepareRequest {
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub asset_id: u64,
}

/// Response with group ID for client signing (Stage 1)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlgorandPrepareResponse {
    /// Unsigned ASA transfer for client to sign
    pub unsigned_transaction: String,

    /// Transaction ID (needed for verification)
    pub tx_id: String,

    /// Atomic group ID (must be included in signed tx)
    pub group_id: String,

    /// Validity window
    pub first_valid: u64,
    pub last_valid: u64,
}
```

---

## 4. Provider Implementation

### 4.1 AlgorandProvider Struct (`src/chain/algorand.rs`)

```rust
use algonaut::algod::v2::Algod;
use algonaut::indexer::v2::Indexer;
use algonaut::core::{Address, Transaction, SignedTransaction};
use algonaut::crypto::Signature;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AlgorandProvider {
    /// Algorand node client (algod)
    algod: Algod,

    /// Indexer client for transaction lookup
    indexer: Indexer,

    /// Facilitator's signing keypair (Ed25519)
    facilitator_keypair: ed25519_dalek::SigningKey,

    /// Facilitator's Algorand address
    facilitator_address: Address,

    /// Network identifier
    network: Network,

    /// USDC ASA ID for this network
    usdc_asset_id: u64,

    /// Transaction ID cache for replay protection
    /// Key: tx_id, Value: expiration_round
    tx_cache: Arc<RwLock<HashMap<String, u64>>>,

    /// Genesis ID for network identification
    genesis_id: String,

    /// Genesis hash for transaction signing
    genesis_hash: [u8; 32],
}
```

### 4.2 Trait Implementations

```rust
// Facilitator trait
impl Facilitator for AlgorandProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error>;
    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error>;
    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error>;
    fn blacklist_info(&self) -> &'static str;
}

// NetworkProviderOps trait
impl NetworkProviderOps for AlgorandProvider {
    fn signer_address(&self) -> String {
        self.facilitator_address.to_string()
    }

    fn network(&self) -> Network {
        self.network.clone()
    }
}

// FromEnvByNetworkBuild trait
impl FromEnvByNetworkBuild for AlgorandProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let algod_url = std::env::var(algod_env_name_from_network(network)?)?;
        let algod_token = std::env::var(algod_token_env_name(network)?).ok();
        let indexer_url = std::env::var(indexer_env_name_from_network(network)?)?;

        let private_key = match network {
            Network::Algorand => std::env::var("ALGORAND_PRIVATE_KEY_MAINNET")?,
            Network::AlgorandTestnet => std::env::var("ALGORAND_PRIVATE_KEY_TESTNET")?,
            _ => return Ok(None),
        };

        let algod = Algod::new(&algod_url, algod_token.as_deref().unwrap_or(""))?;
        let indexer = Indexer::new(&indexer_url)?;

        // Parse 32-byte Ed25519 private key
        let keypair = ed25519_dalek::SigningKey::from_bytes(
            &base64::decode(&private_key)?[..32].try_into()?
        );

        let facilitator_address = Address::from_public_key(&keypair.verifying_key().to_bytes());

        // Get network params
        let params = algod.transaction_params().await?;
        let genesis_hash: [u8; 32] = base64::decode(&params.genesis_hash)?
            .try_into()
            .map_err(|_| "Invalid genesis hash")?;

        let usdc_asset_id = match network {
            Network::Algorand => USDC_ALGORAND_MAINNET,
            Network::AlgorandTestnet => USDC_ALGORAND_TESTNET,
            _ => return Ok(None),
        };

        Ok(Some(Self {
            algod,
            indexer,
            facilitator_keypair: keypair,
            facilitator_address,
            network,
            usdc_asset_id,
            tx_cache: Arc::new(RwLock::new(HashMap::new())),
            genesis_id: params.genesis_id,
            genesis_hash,
        }))
    }
}
```

---

## 5. Verification Flow

### 5.1 verify() Implementation

```rust
async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
    let payload = match &request.payment_payload.payload {
        ExactPaymentPayload::Algorand(p) => p,
        _ => return Err(FacilitatorLocalError::PayloadMismatch),
    };

    // Step 1: Decode signed transaction
    let signed_tx = self.decode_signed_transaction(&payload.signed_transaction)?;

    // Step 2: Validate transaction structure
    self.validate_transaction_structure(&signed_tx, payload)?;

    // Step 3: Check round validity window
    self.validate_round_window(payload.first_valid, payload.last_valid).await?;

    // Step 4: Verify transaction ID matches
    self.verify_tx_id(&signed_tx, &payload.tx_id)?;

    // Step 5: Verify group ID matches
    self.verify_group_id(&signed_tx, &payload.group_id)?;

    // Step 6: Verify cryptographic signature
    self.verify_transaction_signature(&signed_tx, &payload.from)?;

    // Step 7: Check for replay (tx_id not already submitted)
    self.check_tx_not_submitted(&payload.tx_id).await?;

    // Step 8: Verify sender has sufficient USDC balance
    self.verify_usdc_balance(&payload.from, payload.amount).await?;

    // Step 9: Verify sender has opted into USDC ASA
    self.verify_asset_opted_in(&payload.from, payload.asset_id).await?;

    Ok(VerifyResponse::Valid {
        payer: payload.from.clone(),
    })
}
```

### 5.2 Validation Details

**Round Window Validation**:
```rust
async fn validate_round_window(
    &self,
    first_valid: u64,
    last_valid: u64,
) -> Result<(), FacilitatorLocalError> {
    let status = self.algod.status().await?;
    let current_round = status.last_round;

    // Check not expired
    if last_valid < current_round {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::RoundExpired {
            last_valid,
            current_round,
        }));
    }

    // Check not too early
    if first_valid > current_round + 10 {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::RoundTooEarly {
            first_valid,
            current_round,
        }));
    }

    // Check window not too wide (max 1000 rounds ~ 1 hour)
    let window = last_valid.saturating_sub(first_valid);
    if window > 1000 {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::RoundWindowTooWide {
            window,
            max: 1000,
        }));
    }

    Ok(())
}
```

**Transaction Structure Validation**:
```rust
fn validate_transaction_structure(
    &self,
    signed_tx: &SignedTransaction,
    payload: &ExactAlgorandPayload,
) -> Result<(), FacilitatorLocalError> {
    let tx = &signed_tx.transaction;

    // Must be asset transfer
    let asset_transfer = match &tx.asset_transfer {
        Some(at) => at,
        None => return Err(FacilitatorLocalError::Algorand(
            AlgorandError::InvalidTransactionType("Expected asset transfer".into())
        )),
    };

    // Validate fields match payload
    if tx.sender.to_string() != payload.from {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::SenderMismatch {
            expected: payload.from.clone(),
            actual: tx.sender.to_string(),
        }));
    }

    if asset_transfer.receiver.to_string() != payload.to {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::ReceiverMismatch {
            expected: payload.to.clone(),
            actual: asset_transfer.receiver.to_string(),
        }));
    }

    if asset_transfer.amount != payload.amount {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::AmountMismatch {
            expected: payload.amount,
            actual: asset_transfer.amount,
        }));
    }

    if asset_transfer.asset_id != payload.asset_id {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::AssetIdMismatch {
            expected: payload.asset_id,
            actual: asset_transfer.asset_id,
        }));
    }

    // Validate USDC asset ID
    if asset_transfer.asset_id != self.usdc_asset_id {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::InvalidAssetId {
            expected: self.usdc_asset_id,
            actual: asset_transfer.asset_id,
        }));
    }

    Ok(())
}
```

**Signature Verification**:
```rust
fn verify_transaction_signature(
    &self,
    signed_tx: &SignedTransaction,
    from: &str,
) -> Result<(), FacilitatorLocalError> {
    // Decode sender address to get public key
    let sender_address = Address::from_string(from)?;
    let public_key_bytes = sender_address.to_public_key_bytes();

    // Get signature from signed transaction
    let signature = match &signed_tx.signature {
        Some(sig) => sig,
        None => return Err(FacilitatorLocalError::Algorand(
            AlgorandError::MissingSignature
        )),
    };

    // Compute transaction bytes to verify
    let tx_bytes = signed_tx.transaction.bytes_to_sign()?;

    // Verify Ed25519 signature
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&public_key_bytes)?;
    let sig = ed25519_dalek::Signature::from_bytes(&signature.0)?;

    verifying_key.verify(&tx_bytes, &sig)
        .map_err(|_| FacilitatorLocalError::Algorand(AlgorandError::InvalidSignature {
            address: from.to_string(),
        }))?;

    Ok(())
}
```

**USDC Balance Check**:
```rust
async fn verify_usdc_balance(
    &self,
    address: &str,
    amount: u64,
) -> Result<(), FacilitatorLocalError> {
    let account_info = self.algod.account_information(&Address::from_string(address)?).await?;

    // Find USDC asset holding
    let usdc_holding = account_info.assets.iter()
        .find(|a| a.asset_id == self.usdc_asset_id);

    let balance = match usdc_holding {
        Some(h) => h.amount,
        None => return Err(FacilitatorLocalError::Algorand(
            AlgorandError::NotOptedInToAsset {
                address: address.to_string(),
                asset_id: self.usdc_asset_id,
            }
        )),
    };

    if balance < amount {
        return Err(FacilitatorLocalError::Algorand(AlgorandError::InsufficientBalance {
            address: address.to_string(),
            balance,
            required: amount,
        }));
    }

    Ok(())
}
```

---

## 6. Settlement Flow

### 6.1 settle() Implementation

```rust
async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
    let payload = match &request.payment_payload.payload {
        ExactPaymentPayload::Algorand(p) => p,
        _ => return Err(FacilitatorLocalError::PayloadMismatch),
    };

    // Step 1: Re-verify (defensive check)
    self.verify(request).await?;

    // Step 2: Decode client's signed transaction
    let client_signed_tx = self.decode_signed_transaction(&payload.signed_transaction)?;

    // Step 3: Build facilitator's fee payment transaction
    let fee_tx = self.build_fee_transaction(payload).await?;

    // Step 4: Sign facilitator's transaction
    let signed_fee_tx = self.sign_transaction(&fee_tx)?;

    // Step 5: Combine into atomic group
    let signed_group = vec![client_signed_tx, signed_fee_tx];

    // Step 6: Submit atomic group to network
    let tx_id = self.algod.broadcast_signed_transactions(&signed_group).await
        .map_err(|e| FacilitatorLocalError::Algorand(AlgorandError::SubmissionFailed {
            source: e.to_string(),
        }))?;

    // Step 7: Mark transaction as submitted
    {
        let mut cache = self.tx_cache.write().await;
        cache.insert(payload.tx_id.clone(), payload.last_valid);
    }

    // Step 8: Wait for confirmation (optional, can be async)
    let pending = self.algod.pending_transaction_information(&tx_id).await?;

    Ok(SettleResponse {
        success: true,
        transaction: tx_id,
        network: self.network.clone(),
    })
}
```

### 6.2 Fee Transaction Construction

```rust
async fn build_fee_transaction(
    &self,
    payload: &ExactAlgorandPayload,
) -> Result<Transaction, FacilitatorLocalError> {
    let params = self.algod.transaction_params().await?;

    // Build a minimal payment transaction (0 ALGO)
    // This is needed to complete the atomic group
    let fee_tx = Transaction::Payment {
        sender: self.facilitator_address.clone(),
        receiver: Address::from_string(&payload.from)?,  // Pay to client (optional rebate)
        amount: 0,  // No payment, just completing the group
        first_valid: payload.first_valid,
        last_valid: payload.last_valid,
        genesis_id: self.genesis_id.clone(),
        genesis_hash: self.genesis_hash,
        fee: params.min_fee,
        note: Some(b"x402-facilitator".to_vec()),
        group: Some(base64::decode(&payload.group_id)?),  // Same group ID
        ..Default::default()
    };

    Ok(fee_tx)
}

fn sign_transaction(
    &self,
    tx: &Transaction,
) -> Result<SignedTransaction, FacilitatorLocalError> {
    // Compute bytes to sign
    let bytes_to_sign = tx.bytes_to_sign()?;

    // Sign with facilitator's key
    let signature = self.facilitator_keypair.sign(&bytes_to_sign);

    Ok(SignedTransaction {
        transaction: tx.clone(),
        signature: Some(Signature(signature.to_bytes())),
        ..Default::default()
    })
}
```

### 6.3 Group ID Computation

```rust
fn compute_group_id(transactions: &[Transaction]) -> [u8; 32] {
    use sha2::{Sha512_256, Digest};

    // Compute each transaction's ID
    let tx_ids: Vec<[u8; 32]> = transactions.iter()
        .map(|tx| tx.id())
        .collect();

    // Concatenate all IDs with "TG" prefix
    let mut hasher = Sha512_256::new();
    hasher.update(b"TG");  // "Transaction Group" prefix
    for id in tx_ids {
        hasher.update(id);
    }

    hasher.finalize().into()
}
```

---

## 7. Replay Protection

### 7.1 Multi-Layer Protection Strategy

Algorand requires more careful replay protection than EVM due to round-based validity:

```rust
/// Check if transaction has already been submitted
async fn check_tx_not_submitted(&self, tx_id: &str) -> Result<(), FacilitatorLocalError> {
    // Layer 1: Check local cache
    {
        let cache = self.tx_cache.read().await;
        if cache.contains_key(tx_id) {
            return Err(FacilitatorLocalError::Algorand(AlgorandError::TransactionReplay {
                tx_id: tx_id.to_string(),
            }));
        }
    }

    // Layer 2: Check indexer for already-confirmed transactions
    match self.indexer.transaction_information(tx_id).await {
        Ok(_) => {
            // Transaction exists on chain
            return Err(FacilitatorLocalError::Algorand(AlgorandError::TransactionReplay {
                tx_id: tx_id.to_string(),
            }));
        }
        Err(e) if e.is_not_found() => {
            // Good - transaction not found
        }
        Err(e) => {
            tracing::warn!("Indexer lookup failed: {}", e);
            // Continue anyway - rely on local cache
        }
    }

    // Layer 3: Check pending pool
    match self.algod.pending_transaction_information(tx_id).await {
        Ok(pending) if pending.confirmed_round.is_some() => {
            return Err(FacilitatorLocalError::Algorand(AlgorandError::TransactionReplay {
                tx_id: tx_id.to_string(),
            }));
        }
        Ok(pending) if pending.pool_error.is_none() => {
            // Transaction is pending - consider it replay
            return Err(FacilitatorLocalError::Algorand(AlgorandError::TransactionReplay {
                tx_id: tx_id.to_string(),
            }));
        }
        _ => {
            // Not found or error - OK to proceed
        }
    }

    Ok(())
}
```

### 7.2 Cache Management

```rust
/// Periodic cleanup of expired transaction IDs
async fn cleanup_expired_transactions(&self, current_round: u64) {
    let mut cache = self.tx_cache.write().await;
    cache.retain(|_, expiry_round| *expiry_round > current_round);
}

/// Background task for cache maintenance
async fn start_cache_cleanup_task(provider: Arc<AlgorandProvider>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        let status = match provider.algod.status().await {
            Ok(s) => s,
            Err(_) => continue,
        };

        provider.cleanup_expired_transactions(status.last_round).await;
    }
}
```

### 7.3 Production Redis Integration

```rust
async fn check_and_mark_tx_redis(
    redis: &RedisClient,
    tx_id: &str,
    last_valid: u64,
    current_round: u64,
) -> Result<(), AlgorandError> {
    let key = format!("algorand:tx:{}", tx_id);

    // Set NX (only if not exists) with TTL based on round expiry
    let ttl_seconds = ((last_valid.saturating_sub(current_round)) * 4) as usize;  // ~4 sec/round

    let was_set: bool = redis.set_nx_ex(&key, "1", ttl_seconds).await?;

    if !was_set {
        return Err(AlgorandError::TransactionReplay { tx_id: tx_id.to_string() });
    }

    Ok(())
}
```

---

## 8. Error Handling

### 8.1 Algorand-Specific Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum AlgorandError {
    #[error("Invalid transaction encoding: {0}")]
    InvalidEncoding(String),

    #[error("Transaction expired at round {last_valid} (current: {current_round})")]
    RoundExpired {
        last_valid: u64,
        current_round: u64,
    },

    #[error("Transaction not yet valid: first_valid {first_valid} > current {current_round}")]
    RoundTooEarly {
        first_valid: u64,
        current_round: u64,
    },

    #[error("Round window too wide: {window} > max {max}")]
    RoundWindowTooWide {
        window: u64,
        max: u64,
    },

    #[error("Invalid transaction type: {0}")]
    InvalidTransactionType(String),

    #[error("Sender mismatch: expected {expected}, got {actual}")]
    SenderMismatch {
        expected: String,
        actual: String,
    },

    #[error("Receiver mismatch: expected {expected}, got {actual}")]
    ReceiverMismatch {
        expected: String,
        actual: String,
    },

    #[error("Amount mismatch: expected {expected}, got {actual}")]
    AmountMismatch {
        expected: u64,
        actual: u64,
    },

    #[error("Asset ID mismatch: expected {expected}, got {actual}")]
    AssetIdMismatch {
        expected: u64,
        actual: u64,
    },

    #[error("Invalid asset ID: expected USDC {expected}, got {actual}")]
    InvalidAssetId {
        expected: u64,
        actual: u64,
    },

    #[error("Missing signature on transaction")]
    MissingSignature,

    #[error("Invalid signature for address {address}")]
    InvalidSignature { address: String },

    #[error("Transaction {tx_id} already submitted (replay attempt)")]
    TransactionReplay { tx_id: String },

    #[error("Address {address} not opted into asset {asset_id}")]
    NotOptedInToAsset {
        address: String,
        asset_id: u64,
    },

    #[error("Insufficient balance: {address} has {balance}, needs {required}")]
    InsufficientBalance {
        address: String,
        balance: u64,
        required: u64,
    },

    #[error("Transaction ID mismatch: expected {expected}, got {actual}")]
    TxIdMismatch {
        expected: String,
        actual: String,
    },

    #[error("Group ID mismatch: expected {expected}, got {actual}")]
    GroupIdMismatch {
        expected: String,
        actual: String,
    },

    #[error("Transaction submission failed: {source}")]
    SubmissionFailed { source: String },

    #[error("Algod RPC error: {0}")]
    AlgodError(String),

    #[error("Indexer error: {0}")]
    IndexerError(String),
}
```

### 8.2 HTTP Status Code Mapping

| Error | HTTP Status |
|-------|-------------|
| `InvalidEncoding` | 400 Bad Request |
| `RoundExpired` | 400 Bad Request |
| `RoundTooEarly` | 400 Bad Request |
| `RoundWindowTooWide` | 400 Bad Request |
| `InvalidTransactionType` | 400 Bad Request |
| `SenderMismatch` | 400 Bad Request |
| `ReceiverMismatch` | 400 Bad Request |
| `AmountMismatch` | 400 Bad Request |
| `InvalidSignature` | 400 Bad Request |
| `TransactionReplay` | 409 Conflict |
| `NotOptedInToAsset` | 400 Bad Request |
| `InsufficientBalance` | 400 Bad Request |
| `SubmissionFailed` | 500 Internal Server Error |
| `AlgodError` | 503 Service Unavailable |
| `IndexerError` | 503 Service Unavailable |

---

## 9. Environment Configuration

### 9.1 New Environment Variables

```bash
# .env.example additions

# Algorand Mainnet
ALGOD_URL_MAINNET=https://mainnet-api.algonode.cloud
ALGOD_TOKEN_MAINNET=           # Optional for public nodes
INDEXER_URL_MAINNET=https://mainnet-idx.algonode.cloud
ALGORAND_PRIVATE_KEY_MAINNET=  # Base64-encoded 32-byte Ed25519 key

# Algorand Testnet
ALGOD_URL_TESTNET=https://testnet-api.algonode.cloud
ALGOD_TOKEN_TESTNET=           # Optional for public nodes
INDEXER_URL_TESTNET=https://testnet-idx.algonode.cloud
ALGORAND_PRIVATE_KEY_TESTNET=  # Base64-encoded 32-byte Ed25519 key
```

### 9.2 Environment Loading (`src/from_env.rs`)

```rust
pub const ALGOD_URL_MAINNET: &str = "ALGOD_URL_MAINNET";
pub const ALGOD_URL_TESTNET: &str = "ALGOD_URL_TESTNET";
pub const ALGOD_TOKEN_MAINNET: &str = "ALGOD_TOKEN_MAINNET";
pub const ALGOD_TOKEN_TESTNET: &str = "ALGOD_TOKEN_TESTNET";
pub const INDEXER_URL_MAINNET: &str = "INDEXER_URL_MAINNET";
pub const INDEXER_URL_TESTNET: &str = "INDEXER_URL_TESTNET";
pub const ALGORAND_PRIVATE_KEY_MAINNET: &str = "ALGORAND_PRIVATE_KEY_MAINNET";
pub const ALGORAND_PRIVATE_KEY_TESTNET: &str = "ALGORAND_PRIVATE_KEY_TESTNET";

pub fn algod_env_name_from_network(network: Network) -> Option<&'static str> {
    match network {
        Network::Algorand => Some(ALGOD_URL_MAINNET),
        Network::AlgorandTestnet => Some(ALGOD_URL_TESTNET),
        _ => None,
    }
}

pub fn algod_token_env_name(network: Network) -> Option<&'static str> {
    match network {
        Network::Algorand => Some(ALGOD_TOKEN_MAINNET),
        Network::AlgorandTestnet => Some(ALGOD_TOKEN_TESTNET),
        _ => None,
    }
}

pub fn indexer_env_name_from_network(network: Network) -> Option<&'static str> {
    match network {
        Network::Algorand => Some(INDEXER_URL_MAINNET),
        Network::AlgorandTestnet => Some(INDEXER_URL_TESTNET),
        _ => None,
    }
}

pub fn algorand_private_key_env_name(network: Network) -> Option<&'static str> {
    match network {
        Network::Algorand => Some(ALGORAND_PRIVATE_KEY_MAINNET),
        Network::AlgorandTestnet => Some(ALGORAND_PRIVATE_KEY_TESTNET),
        _ => None,
    }
}
```

---

## 10. Integration Steps

### Phase 1: Core Backend (Week 1)

```
Day 1-2: Type Definitions
[ ] Add NetworkFamily::Algorand to src/network.rs
[ ] Add Network::Algorand and Network::AlgorandTestnet
[ ] Add USDC_ALGORAND constants (ASA IDs)
[ ] Add ExactAlgorandPayload to src/types.rs
[ ] Add AlgorandPrepareRequest/Response types
[ ] Update USDCDeployment::by_network()

Day 3-4: Provider Skeleton
[ ] Create src/chain/algorand.rs
[ ] Define AlgorandProvider struct
[ ] Implement FromEnvByNetworkBuild
[ ] Add to NetworkProvider enum in src/chain/mod.rs
[ ] Update dispatch logic in NetworkProvider::verify/settle

Day 5: Transaction Handling
[ ] Implement decode_signed_transaction()
[ ] Implement compute_group_id()
[ ] Implement verify_transaction_signature()
[ ] Add unit tests for msgpack parsing
```

### Phase 2: Verification & Settlement (Week 2)

```
Day 1-2: Verification
[ ] Implement AlgorandProvider::verify()
[ ] Implement validate_round_window()
[ ] Implement validate_transaction_structure()
[ ] Implement verify_usdc_balance()
[ ] Implement verify_asset_opted_in()
[ ] Add unit tests for verification

Day 3-4: Settlement
[ ] Implement AlgorandProvider::settle()
[ ] Implement build_fee_transaction()
[ ] Implement sign_transaction()
[ ] Implement atomic group submission
[ ] Add unit tests for settlement

Day 5: Replay Protection
[ ] Implement check_tx_not_submitted()
[ ] Implement tx_cache management
[ ] Add cleanup_expired_transactions()
[ ] Add background cleanup task
```

### Phase 3: Testing & Integration (Week 3)

```
Day 1-2: Integration Tests
[ ] Create tests/integration/test_algorand_payment.py
[ ] Test verify() with valid signed transaction
[ ] Test verify() with expired round
[ ] Test verify() with invalid signature
[ ] Test settle() end-to-end on testnet

Day 3: Frontend Integration
[ ] Add algorand.png logo to static/images/
[ ] Add logo handler to src/handlers.rs
[ ] Add Algorand network cards to static/index.html

Day 4: Environment & Deployment
[ ] Add algod/indexer URLs to AWS Secrets Manager
[ ] Fund testnet facilitator wallet (ALGO + USDC opt-in)
[ ] Update .env.example
[ ] Update README.md

Day 5: Documentation
[ ] Create guides/ALGORAND_CLIENT_GUIDE.md
[ ] Update ADDING_NEW_CHAINS.md with Algorand example
```

### Phase 4: Production Hardening (Week 4)

```
[ ] Migrate tx_cache to Redis
[ ] Add CloudWatch metrics (latency, error rates)
[ ] Add facilitator ALGO balance monitoring
[ ] Add alerting for low balance (<5 ALGO)
[ ] Security review (signature verification, replay protection)
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
    fn test_decode_signed_transaction() {
        // Test msgpack decoding with known test vectors
        let encoded = "...base64...";
        let result = decode_signed_transaction(encoded);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compute_group_id() {
        // Test group ID computation matches algod
        let tx1 = Transaction::default();
        let tx2 = Transaction::default();
        let group_id = compute_group_id(&[tx1, tx2]);
        assert_eq!(group_id.len(), 32);
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
    fn test_round_validation() {
        // Test round window validation
    }

    #[test]
    fn test_tx_replay_detection() {
        // Test replay protection cache
    }
}
```

### 11.2 Integration Tests

```python
# tests/integration/test_algorand_payment.py

def test_algorand_verify_valid():
    """Test verification of valid Algorand signed transaction."""
    payload = create_test_algorand_payload()
    response = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert response.status_code == 200
    assert response.json()["valid"] == True

def test_algorand_verify_expired():
    """Test rejection of expired round window."""
    payload = create_expired_algorand_payload()
    response = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert response.status_code == 400
    assert "expired" in response.json()["error"].lower()

def test_algorand_verify_wrong_asset():
    """Test rejection of non-USDC asset transfer."""
    payload = create_wrong_asset_payload()
    response = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert response.status_code == 400
    assert "asset" in response.json()["error"].lower()

def test_algorand_settle_success():
    """Test successful settlement on testnet."""
    payload = create_test_algorand_payload()
    response = requests.post(f"{FACILITATOR_URL}/settle", json=payload)
    assert response.status_code == 200
    assert response.json()["success"] == True
    assert response.json()["transaction"] is not None

def test_algorand_replay_protection():
    """Test rejection of duplicate transaction ID."""
    payload = create_test_algorand_payload()
    # First submission succeeds
    response1 = requests.post(f"{FACILITATOR_URL}/settle", json=payload)
    assert response1.status_code == 200
    # Second submission fails
    response2 = requests.post(f"{FACILITATOR_URL}/settle", json=payload)
    assert response2.status_code == 409

def test_algorand_insufficient_balance():
    """Test rejection when sender lacks USDC balance."""
    payload = create_insufficient_balance_payload()
    response = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert response.status_code == 400
    assert "balance" in response.json()["error"].lower()
```

### 11.3 Load Testing

```bash
# k6 load test
k6 run --vus 10 --duration 5m tests/load/algorand_load_test.js
```

Target metrics:
- p99 latency: <3 seconds (Algorand is faster than EVM)
- Error rate: <0.1%
- Throughput: >20 TPS

---

## 12. Production Considerations

### 12.1 Facilitator Wallet Management

**ALGO Balance Requirements**:
- Minimum reserve: 0.1 ALGO (account minimum)
- Recommended: 10 ALGO
- Average fee per transaction: 0.001 ALGO
- Must be opted into USDC ASA (optional for facilitator, but useful)

**Wallet Setup**:
```bash
# Create facilitator wallet
goal account new -d ~/node/data facilitator

# Fund with ALGO
goal clerk send -d ~/node/data -a 10000000 -f FUNDER -t FACILITATOR

# Export private key (base64)
goal account export -d ~/node/data -a FACILITATOR
```

**Monitoring**:
```rust
async fn check_facilitator_balance(algod: &Algod, address: &Address) -> Result<u64> {
    let account = algod.account_information(address).await?;
    let algo_balance = account.amount;

    if algo_balance < 1_000_000 {  // 1 ALGO in microALGO
        tracing::warn!("Facilitator ALGO balance low: {} microALGO", algo_balance);
    }

    Ok(algo_balance)
}
```

### 12.2 Observability

**Metrics**:
- `algorand_verify_latency_ms` - Verification time
- `algorand_settle_latency_ms` - Settlement time
- `algorand_tx_cache_size` - Cache size
- `algorand_expired_rounds_total` - Expired rejections
- `algorand_replay_rejections_total` - Replay attempts
- `algorand_balance_insufficient_total` - Balance failures

**Logs**:
```rust
tracing::info!(
    network = %self.network,
    from = %payload.from,
    to = %payload.to,
    amount = %payload.amount,
    asset_id = %payload.asset_id,
    tx_id = %tx_id,
    "Algorand payment settled"
);
```

### 12.3 Security Hardening

**Input Validation**:
- Reject msgpack payloads > 10KB
- Validate address format (58-char base32)
- Enforce minimum amount (e.g., 0.01 USDC = 10000 microUSDC)
- Enforce maximum amount (e.g., 10,000 USDC)
- Validate round window not too far in future

**Rate Limiting**:
- Per-address: 10 settle requests/minute
- Global: 100 settle requests/minute

### 12.4 Node Provider Selection

| Provider | Rate Limit | Recommended For |
|----------|------------|-----------------|
| algonode.cloud (free) | ~10 req/s | Development/Testing |
| PureStake | Higher | Production |
| Algorand Foundation | Varies | Production |
| Self-hosted | Unlimited | High-volume production |

**AlgoNode Public Endpoints**:
- Mainnet Algod: `https://mainnet-api.algonode.cloud`
- Mainnet Indexer: `https://mainnet-idx.algonode.cloud`
- Testnet Algod: `https://testnet-api.algonode.cloud`
- Testnet Indexer: `https://testnet-idx.algonode.cloud`

---

## Appendix A: Client Integration Example (Two-Stage Protocol)

**IMPORTANT**: Algorand atomic transfers require the facilitator to construct the group ID because:
1. The client does NOT know the facilitator's address
2. The client should NOT construct the facilitator's transaction
3. Group ID depends on ALL transactions in the group

This two-stage protocol ensures security and proper separation of concerns.

### Stage 1: Prepare Request

The client requests a prepared transaction from the facilitator:

```
POST /algorand/prepare
Content-Type: application/json

{
    "from": "SENDER_ADDRESS...",
    "to": "RECIPIENT_ADDRESS...",
    "amount": 1000000,
    "assetId": 31566704
}

Response:
{
    "unsignedTransaction": "base64...",  // Client's ASA transfer (without signature)
    "txId": "TXID...",                   // Transaction ID for verification
    "groupId": "base64...",              // Computed group ID
    "firstValid": 12345678,
    "lastValid": 12345778
}
```

### Stage 2: Sign and Submit

The client signs the prepared transaction and submits to `/settle`.

### JavaScript/TypeScript Client

```typescript
import algosdk from 'algosdk';

const USDC_ASA_ID = 31566704;  // Mainnet USDC

async function createAlgorandPayment(
    from: string,
    to: string,
    amount: number,
    senderMnemonic: string,
    facilitatorUrl: string
): Promise<PaymentResult> {
    // Stage 1: Request prepared transaction from facilitator
    const prepareResponse = await fetch(`${facilitatorUrl}/algorand/prepare`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            from,
            to,
            amount,
            assetId: USDC_ASA_ID,
        }),
    });

    const prepared = await prepareResponse.json();
    // prepared = { unsignedTransaction, txId, groupId, firstValid, lastValid }

    // Stage 2: Decode and sign the prepared transaction
    const unsignedTxBytes = Buffer.from(prepared.unsignedTransaction, 'base64');
    const unsignedTx = algosdk.decodeUnsignedTransaction(unsignedTxBytes);

    // Sign with client's key
    const senderAccount = algosdk.mnemonicToSecretKey(senderMnemonic);
    const signedTx = unsignedTx.signTxn(senderAccount.sk);

    // Encode signed transaction
    const signedTxBase64 = Buffer.from(signedTx).toString('base64');

    // Stage 3: Submit to facilitator for settlement
    const response = await fetch(`${facilitatorUrl}/settle`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            x402Version: 'v1',
            scheme: 'exact',
            network: 'algorand',
            payload: {
                payloadType: 'algorand',
                from,
                to,
                amount,
                assetId: USDC_ASA_ID,
                signedTransaction: signedTxBase64,
                txId: prepared.txId,
                firstValid: prepared.firstValid,
                lastValid: prepared.lastValid,
                groupId: prepared.groupId,
            },
        }),
    });

    return response.json();
}
```

### Rust Client

```rust
use reqwest::Client;
use serde::{Deserialize, Serialize};

const USDC_ASA_ID: u64 = 31566704;

#[derive(Deserialize)]
struct PrepareResponse {
    unsigned_transaction: String,
    tx_id: String,
    group_id: String,
    first_valid: u64,
    last_valid: u64,
}

async fn create_algorand_payment(
    from: &str,
    to: &str,
    amount: u64,
    sender_keypair: &ed25519_dalek::SigningKey,
    facilitator_url: &str,
) -> Result<PaymentResult, Error> {
    let client = Client::new();

    // Stage 1: Request prepared transaction from facilitator
    let prepare_response: PrepareResponse = client
        .post(&format!("{}/algorand/prepare", facilitator_url))
        .json(&serde_json::json!({
            "from": from,
            "to": to,
            "amount": amount,
            "assetId": USDC_ASA_ID,
        }))
        .send()
        .await?
        .json()
        .await?;

    // Stage 2: Decode the unsigned transaction
    let unsigned_tx_bytes = base64::decode(&prepare_response.unsigned_transaction)?;
    let unsigned_tx: Transaction = rmp_serde::from_slice(&unsigned_tx_bytes)?;

    // Sign with client's key
    let bytes_to_sign = unsigned_tx.bytes_to_sign()?;
    let signature = sender_keypair.sign(&bytes_to_sign);

    let signed_tx = SignedTransaction {
        transaction: unsigned_tx,
        signature: Some(Signature(signature.to_bytes())),
        ..Default::default()
    };

    // Encode signed transaction
    let signed_tx_bytes = rmp_serde::to_vec(&signed_tx)?;
    let signed_tx_base64 = base64::encode(&signed_tx_bytes);

    // Stage 3: Submit to facilitator for settlement
    let response = client
        .post(&format!("{}/settle", facilitator_url))
        .json(&serde_json::json!({
            "x402Version": "v1",
            "scheme": "exact",
            "network": "algorand",
            "payload": {
                "payloadType": "algorand",
                "from": from,
                "to": to,
                "amount": amount,
                "assetId": USDC_ASA_ID,
                "signedTransaction": signed_tx_base64,
                "txId": prepare_response.tx_id,
                "firstValid": prepare_response.first_valid,
                "lastValid": prepare_response.last_valid,
                "groupId": prepare_response.group_id,
            }
        }))
        .send()
        .await?;

    response.json().await
}
```

### Facilitator: /algorand/prepare Endpoint

The facilitator must implement the prepare endpoint:

```rust
async fn algorand_prepare(
    State(provider): State<Arc<AlgorandProvider>>,
    Json(request): Json<AlgorandPrepareRequest>,
) -> Result<Json<AlgorandPrepareResponse>, ApiError> {
    // Get current network params
    let params = provider.algod.transaction_params().await?;
    let first_valid = params.last_round;
    let last_valid = params.last_round + 100;  // 100-round window

    // Build client's ASA transfer transaction
    let client_tx = Transaction::AssetTransfer(AssetTransferTransaction {
        sender: Address::from_string(&request.from)?,
        receiver: Address::from_string(&request.to)?,
        amount: request.amount,
        asset_id: request.asset_id,
        first_valid,
        last_valid,
        genesis_id: params.genesis_id.clone(),
        genesis_hash: params.genesis_hash,
        fee: MicroAlgos(0),  // Client pays no fee
        ..Default::default()
    });

    // Build facilitator's fee transaction
    let fee_tx = Transaction::Payment {
        sender: provider.facilitator_address.clone(),
        receiver: Address::from_string(&request.from)?,
        amount: MicroAlgos(0),
        first_valid,
        last_valid,
        genesis_id: params.genesis_id,
        genesis_hash: params.genesis_hash,
        fee: MicroAlgos(2000),  // Facilitator pays both fees
        ..Default::default()
    };

    // Compute group ID
    let group_id = compute_group_id(&[client_tx.clone(), fee_tx]);

    // Add group ID to client's transaction
    let mut client_tx_with_group = client_tx.clone();
    client_tx_with_group.group = Some(group_id);

    // Encode unsigned transaction
    let unsigned_tx_bytes = rmp_serde::to_vec(&client_tx_with_group)?;
    let unsigned_tx_base64 = base64::encode(&unsigned_tx_bytes);

    Ok(Json(AlgorandPrepareResponse {
        unsigned_transaction: unsigned_tx_base64,
        tx_id: client_tx_with_group.id().to_string(),
        group_id: base64::encode(&group_id),
        first_valid,
        last_valid,
    }))
}
```

---

## Appendix B: Algorand Network Constants

```rust
// Genesis IDs
pub const ALGORAND_MAINNET_GENESIS_ID: &str = "mainnet-v1.0";
pub const ALGORAND_TESTNET_GENESIS_ID: &str = "testnet-v1.0";

// USDC ASA IDs
pub const USDC_ALGORAND_MAINNET: u64 = 31566704;
pub const USDC_ALGORAND_TESTNET: u64 = 10458941;

impl Network {
    pub fn algorand_genesis_id(&self) -> Option<&'static str> {
        match self {
            Network::Algorand => Some(ALGORAND_MAINNET_GENESIS_ID),
            Network::AlgorandTestnet => Some(ALGORAND_TESTNET_GENESIS_ID),
            _ => None,
        }
    }

    pub fn algorand_usdc_asa_id(&self) -> Option<u64> {
        match self {
            Network::Algorand => Some(USDC_ALGORAND_MAINNET),
            Network::AlgorandTestnet => Some(USDC_ALGORAND_TESTNET),
            _ => None,
        }
    }
}
```

---

## Summary

This implementation plan provides a complete roadmap for adding Algorand support to x402-rs. Key highlights:

1. **Authorization Model**: Uses Atomic Transfers with pre-signed ASA transfers
2. **Two-Stage Protocol**: REQUIRED - facilitator prepares transaction with group ID, client signs
3. **Replay Protection**: Multi-layer approach (local cache + indexer + pending pool)
4. **Fee Abstraction**: Facilitator completes atomic group and pays network fees
5. **Clean Architecture**: Follows existing provider pattern

**Key Differences from Stellar**:
- No native authorization framework (requires atomic transfers + two-stage protocol)
- Round-based validity (not ledger-based like Stellar)
- MessagePack encoding (not XDR)
- Two-transaction atomic groups (not single-transaction with auth)
- Requires `/algorand/prepare` endpoint (Stellar doesn't need this)

**Estimated Total Effort**: 3-4 weeks for production-ready implementation

**Next Steps**:
1. Review plan with team
2. Set up Algorand testnet environment
3. Fund testnet wallet with ALGO and opt into testnet USDC
4. Begin Phase 1 implementation
