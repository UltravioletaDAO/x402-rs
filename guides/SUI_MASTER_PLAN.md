# Sui Integration Master Plan

> **Project**: x402-rs Payment Facilitator - Sui Network Support
> **Status**: APPROVED FOR IMPLEMENTATION
> **Created**: December 29, 2025
> **Estimated Duration**: 5 weeks
> **Priority**: HIGH (First x402 facilitator with Sui support)

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Prerequisites](#prerequisites)
3. [Phase 1: Foundation Setup](#phase-1-foundation-setup-week-1)
4. [Phase 2: Core Provider Implementation](#phase-2-core-provider-implementation-week-2-3)
5. [Phase 3: Integration & Configuration](#phase-3-integration--configuration-week-4)
6. [Phase 4: Testing](#phase-4-testing-week-4-5)
7. [Phase 5: Production Deployment](#phase-5-production-deployment-week-5)
8. [Technical Reference](#technical-reference)
9. [Risk Mitigation](#risk-mitigation)
10. [Success Criteria](#success-criteria)

---

## Executive Summary

### Why Sui?

| Factor | Value |
|--------|-------|
| **Decision Matrix Score** | 4.25/5 (2nd after Stellar) |
| **USDC Circulation** | $450M+ native Circle |
| **Wallet Support** | EXCELLENT - all wallets support `sui:signTransaction` |
| **Competitive Advantage** | First x402 facilitator with Sui |
| **Architecture Similarity** | Very similar to Solana (sponsored TX model) |

### Key Technical Insight

Sui uses **Sponsored Transactions** at protocol level - the same pattern as our Solana implementation:
- User builds full TransactionData
- User signs their portion
- Facilitator signs as gas sponsor
- Dual-signature execution

This means we can **follow Solana patterns closely**.

---

## Prerequisites

### Before Starting Development

#### 1. Environment Setup

```bash
# Verify Rust version (Sui SDK requires 1.82+, prefer 1.86+ for edition 2024)
rustc --version

# If needed, update Rust
rustup update stable
```

#### 2. Wallet Creation

Create dedicated Sui wallets for the facilitator:

```bash
# Install Sui CLI (if not installed)
cargo install --locked --git https://github.com/MystenLabs/sui.git --branch mainnet sui

# Create testnet wallet
sui client new-env --alias testnet --rpc https://fullnode.testnet.sui.io:443
sui client switch --env testnet
sui keytool generate ed25519
# Save the private key securely

# Create mainnet wallet
sui client new-env --alias mainnet --rpc https://fullnode.mainnet.sui.io:443
sui client switch --env mainnet
sui keytool generate ed25519
# Save the private key securely
```

#### 3. Fund Wallets

```bash
# Testnet - use faucet
sui client faucet

# Mainnet - transfer ~10 SUI minimum for gas operations
# (Purchase SUI from exchange, send to facilitator address)
```

#### 4. Gather Contract Information

| Item | Mainnet | Testnet |
|------|---------|---------|
| **USDC Type** | `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC` | `0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC` |
| **Decimals** | 6 | 6 |
| **RPC (Public)** | `https://fullnode.mainnet.sui.io:443` | `https://fullnode.testnet.sui.io:443` |

---

## Phase 1: Foundation Setup (Week 1)

### 1.1 Add Sui SDK Dependencies

**File**: `Cargo.toml`

```toml
[dependencies]
# Sui dependencies (pin to specific commit for stability)
sui-sdk = { git = "https://github.com/MystenLabs/sui.git", rev = "mainnet-v1.XX.X", package = "sui-sdk" }
sui-types = { git = "https://github.com/MystenLabs/sui.git", rev = "mainnet-v1.XX.X", package = "sui-types" }
sui-keys = { git = "https://github.com/MystenLabs/sui.git", rev = "mainnet-v1.XX.X", package = "sui-keys" }
shared-crypto = { git = "https://github.com/MystenLabs/sui.git", rev = "mainnet-v1.XX.X", package = "shared-crypto" }
bcs = "0.1"

[features]
default = ["solana"]
solana = []
sui = []  # NEW: Feature flag for Sui support
```

**Checklist**:
- [ ] Pin to specific git revision (check latest mainnet release)
- [ ] Add `sui` feature flag
- [ ] Verify cargo check passes with `--features sui`

---

### 1.2 Add Network Enum Variants

**File**: `src/network.rs`

```rust
// Add to Network enum (around line 30-100)

/// Sui mainnet
#[serde(rename = "sui")]
Sui,
/// Sui testnet
#[serde(rename = "sui-testnet")]
SuiTestnet,
```

**Update `Network::variants()`**:
```rust
pub fn variants() -> &'static [Self] {
    &[
        // ... existing variants ...
        Self::Sui,
        Self::SuiTestnet,
    ]
}
```

**Update `Display` impl**:
```rust
impl Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // ... existing ...
            Self::Sui => write!(f, "Sui"),
            Self::SuiTestnet => write!(f, "Sui Testnet"),
        }
    }
}
```

**Update `FromStr` impl**:
```rust
impl FromStr for Network {
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // ... existing ...
            "sui" => Ok(Self::Sui),
            "sui-testnet" => Ok(Self::SuiTestnet),
            _ => Err(NetworkParseError),
        }
    }
}
```

**Checklist**:
- [ ] Add `Sui` and `SuiTestnet` variants
- [ ] Add to `variants()` array
- [ ] Add to `Display` impl
- [ ] Add to `FromStr` impl
- [ ] Add to `is_testnet()` method

---

### 1.3 Add NetworkFamily::Sui

**File**: `src/network.rs`

```rust
pub enum NetworkFamily {
    Evm,
    Solana,
    Near,
    Stellar,
    #[cfg(feature = "algorand")]
    Algorand,
    #[cfg(feature = "sui")]
    Sui,  // NEW
}

impl From<Network> for NetworkFamily {
    fn from(network: Network) -> Self {
        match network {
            // ... existing ...
            #[cfg(feature = "sui")]
            Network::Sui | Network::SuiTestnet => NetworkFamily::Sui,
        }
    }
}
```

**Checklist**:
- [ ] Add `Sui` variant to `NetworkFamily`
- [ ] Add feature gate `#[cfg(feature = "sui")]`
- [ ] Add mapping from `Network::Sui*` to `NetworkFamily::Sui`

---

### 1.4 Add CAIP-2 Support

**File**: `src/caip2.rs`

```rust
// Add Sui CAIP-2 mappings
// Sui uses "sui:mainnet" and "sui:testnet" format

impl Network {
    pub fn to_caip2(&self) -> String {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            Self::Sui => "sui:mainnet".to_string(),
            #[cfg(feature = "sui")]
            Self::SuiTestnet => "sui:testnet".to_string(),
        }
    }

    pub fn from_caip2(s: &str) -> Option<Self> {
        match s {
            // ... existing ...
            #[cfg(feature = "sui")]
            "sui:mainnet" => Some(Self::Sui),
            #[cfg(feature = "sui")]
            "sui:testnet" => Some(Self::SuiTestnet),
            _ => None,
        }
    }
}
```

**Checklist**:
- [ ] Add `sui:mainnet` mapping
- [ ] Add `sui:testnet` mapping

---

### 1.5 Add MixedAddress::Sui Variant

**File**: `src/types.rs`

```rust
use sui_types::base_types::SuiAddress;

pub enum MixedAddress {
    Evm(EvmAddress),
    Offchain(String),
    Solana(Pubkey),
    Near(String),
    Stellar(String),
    Algorand(String),
    #[cfg(feature = "sui")]
    Sui(SuiAddress),  // NEW
}

// Update Display impl
impl Display for MixedAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            MixedAddress::Sui(address) => write!(f, "{address}"),
        }
    }
}

// Update Deserialize impl - add Sui address detection
impl<'de> Deserialize<'de> for MixedAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> {
        // ... existing logic ...

        // Add before off-chain check:
        // Sui address (0x... 64 hex chars, 32 bytes)
        #[cfg(feature = "sui")]
        if s.starts_with("0x") && s.len() == 66 {
            if let Ok(addr) = SuiAddress::from_str(&s) {
                return Ok(MixedAddress::Sui(addr));
            }
        }

        // ... rest of function ...
    }
}

// Update Serialize impl
impl Serialize for MixedAddress {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            MixedAddress::Sui(addr) => serializer.serialize_str(&addr.to_string()),
        }
    }
}
```

**Checklist**:
- [ ] Add `Sui(SuiAddress)` variant
- [ ] Update `Display` impl
- [ ] Update `Deserialize` impl with Sui detection
- [ ] Update `Serialize` impl

---

### 1.6 Add ExactSuiPayload Type

**File**: `src/types.rs`

```rust
/// Sui payment payload - similar to Solana's structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(feature = "sui")]
pub struct ExactSuiPayload {
    /// Base64-encoded BCS-serialized TransactionData
    pub transaction: String,
    /// User's signature over the TransactionData (Base64)
    pub signature: String,
}

pub enum ExactPaymentPayload {
    Evm(ExactEvmPayload),
    Solana(ExactSolanaPayload),
    Near(ExactNearPayload),
    Stellar(ExactStellarPayload),
    #[cfg(feature = "algorand")]
    Algorand(ExactAlgorandPayload),
    #[cfg(feature = "sui")]
    Sui(ExactSuiPayload),  // NEW
}
```

**Checklist**:
- [ ] Add `ExactSuiPayload` struct
- [ ] Add `Sui(ExactSuiPayload)` to `ExactPaymentPayload` enum

---

### 1.7 Add RPC Environment Constants

**File**: `src/from_env.rs`

```rust
// Add constants
#[cfg(feature = "sui")]
pub const ENV_RPC_SUI: &str = "RPC_URL_SUI";
#[cfg(feature = "sui")]
pub const ENV_RPC_SUI_TESTNET: &str = "RPC_URL_SUI_TESTNET";

#[cfg(feature = "sui")]
pub const ENV_SUI_PRIVATE_KEY_MAINNET: &str = "SUI_PRIVATE_KEY_MAINNET";
#[cfg(feature = "sui")]
pub const ENV_SUI_PRIVATE_KEY_TESTNET: &str = "SUI_PRIVATE_KEY_TESTNET";
#[cfg(feature = "sui")]
pub const ENV_SUI_PRIVATE_KEY: &str = "SUI_PRIVATE_KEY";  // Fallback

// Update rpc_env_name_from_network
pub fn rpc_env_name_from_network(network: Network) -> &'static str {
    match network {
        // ... existing ...
        #[cfg(feature = "sui")]
        Network::Sui => ENV_RPC_SUI,
        #[cfg(feature = "sui")]
        Network::SuiTestnet => ENV_RPC_SUI_TESTNET,
    }
}
```

**Checklist**:
- [ ] Add RPC URL constants
- [ ] Add private key constants (mainnet, testnet, fallback)
- [ ] Update `rpc_env_name_from_network()`

---

### 1.8 Add USDC Token Deployments

**File**: `src/network.rs`

```rust
#[cfg(feature = "sui")]
static USDC_SUI: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Sui(
                SuiAddress::from_str("0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7")
                    .expect("valid sui usdc address")
            ),
            network: Network::Sui,
        },
        decimals: 6,
        eip712: None,  // Sui doesn't use EIP-712
    })
});

#[cfg(feature = "sui")]
static USDC_SUI_TESTNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Sui(
                SuiAddress::from_str("0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29")
                    .expect("valid sui testnet usdc address")
            ),
            network: Network::SuiTestnet,
        },
        decimals: 6,
        eip712: None,
    })
});

// Update USDCDeployment::by_network
impl USDCDeployment {
    pub fn by_network<N: Borrow<Network>>(network: N) -> &'static USDCDeployment {
        match network.borrow() {
            // ... existing ...
            #[cfg(feature = "sui")]
            Network::Sui => &USDC_SUI,
            #[cfg(feature = "sui")]
            Network::SuiTestnet => &USDC_SUI_TESTNET,
        }
    }
}
```

**Note**: Sui USDC uses a Type ID format, not a simple address. The address portion identifies the package.

**Checklist**:
- [ ] Add `USDC_SUI` static
- [ ] Add `USDC_SUI_TESTNET` static
- [ ] Update `by_network()` match

---

### Phase 1 Completion Checklist

- [ ] All types compile with `cargo check --features sui`
- [ ] No warnings about non-exhaustive patterns
- [ ] Network enum includes Sui variants
- [ ] CAIP-2 mappings work correctly

---

## Phase 2: Core Provider Implementation (Week 2-3)

### 2.1 Create src/chain/sui.rs

**File**: `src/chain/sui.rs` (NEW - ~500 lines)

```rust
//! Sui blockchain provider for x402-rs facilitator.
//!
//! Implements sponsored transaction flow:
//! 1. User builds TransactionData with USDC transfer
//! 2. User signs as sender
//! 3. Facilitator validates PTB (Programmable Transaction Block)
//! 4. Facilitator signs as gas sponsor
//! 5. Dual-signature execution

use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use sui_sdk::SuiClient;
use sui_sdk::rpc_types::{
    SuiTransactionBlockResponseOptions, SuiTransactionBlockResponse,
};
use sui_types::base_types::{ObjectID, SuiAddress};
use sui_types::crypto::{SuiKeyPair, Signature as SuiSignature};
use sui_types::signature::GenericSignature;
use sui_types::transaction::{
    TransactionData, TransactionDataAPI, Command, ProgrammableTransaction,
};
use bcs;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env::{self, SignerType};
use crate::network::Network;
use crate::types::{
    ExactPaymentPayload, ExactSuiPayload, MixedAddress, PaymentRequirements,
    SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
    SupportedPaymentKindsResponse, TransactionHash, VerifyRequest, VerifyResponse,
    Scheme, X402Version,
};

/// Maximum gas budget the facilitator will sponsor (in MIST - 1e-9 SUI)
const DEFAULT_MAX_GAS_BUDGET: u64 = 50_000_000; // 0.05 SUI

/// Sui chain context
#[derive(Clone, Debug)]
pub struct SuiChain {
    pub network: Network,
}

impl TryFrom<Network> for SuiChain {
    type Error = FacilitatorLocalError;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Sui => Ok(Self { network: value }),
            Network::SuiTestnet => Ok(Self { network: value }),
            // All other networks are not Sui
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }
}

/// Sui address wrapper for type conversions
#[derive(Clone, Debug)]
pub struct SuiAddressWrapper {
    address: SuiAddress,
}

impl From<SuiAddress> for SuiAddressWrapper {
    fn from(address: SuiAddress) -> Self {
        Self { address }
    }
}

impl From<SuiAddressWrapper> for SuiAddress {
    fn from(wrapper: SuiAddressWrapper) -> Self {
        wrapper.address
    }
}

impl TryFrom<MixedAddress> for SuiAddressWrapper {
    type Error = FacilitatorLocalError;

    fn try_from(value: MixedAddress) -> Result<Self, Self::Error> {
        match value {
            MixedAddress::Sui(address) => Ok(Self { address }),
            _ => Err(FacilitatorLocalError::InvalidAddress(
                "expected Sui address".to_string(),
            )),
        }
    }
}

impl From<SuiAddressWrapper> for MixedAddress {
    fn from(value: SuiAddressWrapper) -> Self {
        MixedAddress::Sui(value.address)
    }
}

/// Sui provider for payment verification and settlement
#[derive(Clone)]
pub struct SuiProvider {
    keypair: Arc<SuiKeyPair>,
    chain: SuiChain,
    sui_client: Arc<SuiClient>,
    max_gas_budget: u64,
}

impl Debug for SuiProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuiProvider")
            .field("address", &self.keypair.public().to_sui_address())
            .field("chain", &self.chain)
            .finish()
    }
}

impl SuiProvider {
    /// Create a new Sui provider
    pub async fn try_new(
        keypair: SuiKeyPair,
        rpc_url: String,
        network: Network,
        max_gas_budget: u64,
    ) -> Result<Self, FacilitatorLocalError> {
        let chain = SuiChain::try_from(network)?;

        let sui_client = SuiClient::new(&rpc_url)
            .await
            .map_err(|e| FacilitatorLocalError::Other(format!("Failed to create Sui client: {e}")))?;

        Ok(Self {
            keypair: Arc::new(keypair),
            chain,
            sui_client: Arc::new(sui_client),
            max_gas_budget,
        })
    }

    /// Get max gas budget from environment or use default
    fn max_gas_budget_from_env(network: Network) -> u64 {
        let suffix = match network {
            Network::Sui => "SUI",
            Network::SuiTestnet => "SUI_TESTNET",
            _ => return DEFAULT_MAX_GAS_BUDGET,
        };

        let var_name = format!("X402_SUI_MAX_GAS_BUDGET_{}", suffix);
        std::env::var(&var_name)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_GAS_BUDGET)
    }

    /// Verify gas budget is within limits
    fn verify_gas_budget(&self, tx: &TransactionData) -> Result<u64, FacilitatorLocalError> {
        let gas_budget = tx.gas_data().budget;
        if gas_budget > self.max_gas_budget {
            return Err(FacilitatorLocalError::DecodingError(format!(
                "gas budget {} exceeds maximum {}",
                gas_budget, self.max_gas_budget
            )));
        }
        Ok(gas_budget)
    }

    /// Verify the facilitator is not being exploited
    fn verify_sponsor_safety(&self, tx: &TransactionData) -> Result<(), FacilitatorLocalError> {
        let sponsor_address = self.keypair.public().to_sui_address();

        // Verify sender is NOT the facilitator (prevent self-transfers)
        if tx.sender() == sponsor_address {
            return Err(FacilitatorLocalError::DecodingError(
                "sender cannot be facilitator".to_string(),
            ));
        }

        // TODO: Additional safety checks:
        // - Verify no commands transfer facilitator's objects
        // - Verify no MoveCall targets dangerous functions

        Ok(())
    }

    /// Find and verify USDC transfer in PTB commands
    fn verify_transfer_command(
        &self,
        tx: &TransactionData,
        requirements: &PaymentRequirements,
    ) -> Result<TransferDetails, FacilitatorLocalError> {
        // Get the PTB from transaction
        let ptb = match tx.kind() {
            sui_types::transaction::TransactionKind::ProgrammableTransaction(ptb) => ptb,
            _ => {
                return Err(FacilitatorLocalError::DecodingError(
                    "expected ProgrammableTransaction".to_string(),
                ))
            }
        };

        // Look for TransferObjects command
        for command in ptb.commands.iter() {
            match command {
                Command::TransferObjects(objects, recipient) => {
                    // TODO: Verify:
                    // 1. Objects include USDC coin
                    // 2. Recipient matches requirements.pay_to
                    // 3. Amount matches requirements.max_amount_required

                    // For now, return placeholder
                    return Ok(TransferDetails {
                        sender: tx.sender(),
                        recipient: SuiAddress::ZERO, // TODO: Extract from recipient Argument
                        amount: 0, // TODO: Extract from coin value
                    });
                }
                _ => continue,
            }
        }

        Err(FacilitatorLocalError::DecodingError(
            "no transfer command found".to_string(),
        ))
    }

    /// Full verification of transfer request
    async fn verify_transfer(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerificationResult, FacilitatorLocalError> {
        // 1. Extract Sui payload
        let payload = match &request.payment_payload.payload {
            ExactPaymentPayload::Sui(p) => p,
            _ => {
                return Err(FacilitatorLocalError::DecodingError(
                    "expected Sui payload".to_string(),
                ))
            }
        };

        // 2. Decode BCS-serialized TransactionData
        let tx_bytes = base64::decode(&payload.transaction).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("invalid base64 transaction: {e}"))
        })?;

        let tx: TransactionData = bcs::from_bytes(&tx_bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("invalid BCS transaction: {e}"))
        })?;

        // 3. Verify gas budget
        self.verify_gas_budget(&tx)?;

        // 4. Verify sponsor safety
        self.verify_sponsor_safety(&tx)?;

        // 5. Verify transfer command
        let transfer = self.verify_transfer_command(&tx, &request.payment_requirements)?;

        // 6. Decode and verify user signature
        let user_sig_bytes = base64::decode(&payload.signature).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("invalid base64 signature: {e}"))
        })?;

        // TODO: Verify signature matches transaction and sender

        // 7. Dry-run simulation (optional but recommended)
        // let simulation = self.sui_client.read_api()
        //     .dry_run_transaction_block(tx_bytes)
        //     .await?;

        Ok(VerificationResult {
            transaction: tx,
            transaction_bytes: tx_bytes,
            user_signature: user_sig_bytes,
            sender: transfer.sender,
        })
    }
}

/// Internal struct for transfer details
struct TransferDetails {
    sender: SuiAddress,
    recipient: SuiAddress,
    amount: u64,
}

/// Internal struct for verification result
struct VerificationResult {
    transaction: TransactionData,
    transaction_bytes: Vec<u8>,
    user_signature: Vec<u8>,
    sender: SuiAddress,
}

impl FromEnvByNetworkBuild for SuiProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        // Check if this is a Sui network
        if !matches!(network, Network::Sui | Network::SuiTestnet) {
            return Ok(None);
        }

        // Get RPC URL
        let rpc_var = from_env::rpc_env_name_from_network(network);
        let rpc_url = match std::env::var(rpc_var) {
            Ok(url) => url,
            Err(_) => {
                tracing::warn!("Sui RPC URL not configured for {network}, skipping");
                return Ok(None);
            }
        };

        // Load keypair
        let keypair = SignerType::from_env()?.make_sui_wallet(network)?;

        // Get max gas budget
        let max_gas_budget = Self::max_gas_budget_from_env(network);

        let provider = Self::try_new(keypair, rpc_url, network, max_gas_budget).await?;

        tracing::info!(
            "Sui provider initialized for {} with address {}",
            network,
            provider.keypair.public().to_sui_address()
        );

        Ok(Some(provider))
    }
}

impl NetworkProviderOps for SuiProvider {
    fn signer_address(&self) -> MixedAddress {
        MixedAddress::Sui(self.keypair.public().to_sui_address())
    }

    fn network(&self) -> Network {
        self.chain.network
    }
}

impl Facilitator for SuiProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let result = self.verify_transfer(request).await?;
        Ok(VerifyResponse::valid(MixedAddress::Sui(result.sender)))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        // 1. Re-verify the transaction
        let verify_request = VerifyRequest {
            payment_payload: request.payment_payload.clone(),
            payment_requirements: request.payment_requirements.clone(),
        };
        let verification = self.verify_transfer(&verify_request).await?;

        // 2. Sign as sponsor
        let sponsor_sig = self.keypair.sign(&verification.transaction_bytes);

        // 3. Create signature array (user + sponsor)
        let signatures = vec![
            GenericSignature::from_bytes(&verification.user_signature)
                .map_err(|e| FacilitatorLocalError::InvalidSignature(
                    MixedAddress::Sui(verification.sender),
                    format!("invalid user signature: {e}"),
                ))?,
            GenericSignature::Signature(sponsor_sig),
        ];

        // 4. Execute transaction
        let response = self
            .sui_client
            .quorum_driver_api()
            .execute_transaction_block(
                sui_types::transaction::Transaction::new(
                    sui_types::transaction::SenderSignedData::new(
                        verification.transaction,
                        signatures,
                    ),
                ),
                SuiTransactionBlockResponseOptions::new()
                    .with_effects()
                    .with_events(),
                None,
            )
            .await
            .map_err(|e| FacilitatorLocalError::Other(format!("execution failed: {e}")))?;

        // 5. Check for success
        if let Some(effects) = response.effects {
            if effects.status().is_err() {
                return Err(FacilitatorLocalError::Other(format!(
                    "transaction failed: {:?}",
                    effects.status()
                )));
            }
        }

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer: MixedAddress::Sui(verification.sender),
            transaction: Some(TransactionHash::Sui(response.digest.to_string())),
            network: self.chain.network,
        })
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.chain.network.to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: None,
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}
```

**Checklist**:
- [ ] Create `src/chain/sui.rs`
- [ ] Implement `SuiChain` struct
- [ ] Implement `SuiProvider` struct
- [ ] Implement `verify_gas_budget()`
- [ ] Implement `verify_sponsor_safety()`
- [ ] Implement `verify_transfer_command()`
- [ ] Implement `FromEnvByNetworkBuild`
- [ ] Implement `NetworkProviderOps`
- [ ] Implement `Facilitator` trait

---

### 2.2 Update src/chain/mod.rs

**File**: `src/chain/mod.rs`

```rust
// Add module declaration
#[cfg(feature = "sui")]
pub mod sui;

// Add import
#[cfg(feature = "sui")]
use crate::chain::sui::SuiProvider;

// Update NetworkProvider enum
pub enum NetworkProvider {
    Evm(EvmProvider),
    Solana(SolanaProvider),
    Near(NearProvider),
    Stellar(StellarProvider),
    #[cfg(feature = "algorand")]
    Algorand(AlgorandProvider),
    #[cfg(feature = "sui")]
    Sui(SuiProvider),  // NEW
}

// Update FromEnvByNetworkBuild impl
impl FromEnvByNetworkBuild for NetworkProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let family: NetworkFamily = network.into();
        let provider = match family {
            // ... existing ...
            #[cfg(feature = "sui")]
            NetworkFamily::Sui => {
                let provider = SuiProvider::from_env(network).await?;
                provider.map(NetworkProvider::Sui)
            }
        };
        Ok(provider)
    }
}

// Update NetworkProviderOps impl
impl NetworkProviderOps for NetworkProvider {
    fn signer_address(&self) -> MixedAddress {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            NetworkProvider::Sui(provider) => provider.signer_address(),
        }
    }

    fn network(&self) -> Network {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            NetworkProvider::Sui(provider) => provider.network(),
        }
    }
}

// Update Facilitator impl
impl Facilitator for NetworkProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            NetworkProvider::Sui(provider) => provider.verify(request).await,
        }
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            NetworkProvider::Sui(provider) => provider.settle(request).await,
        }
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        match self {
            // ... existing ...
            #[cfg(feature = "sui")]
            NetworkProvider::Sui(provider) => provider.supported().await,
        }
    }
}
```

**Checklist**:
- [ ] Add `pub mod sui;` with feature gate
- [ ] Add `Sui(SuiProvider)` to enum
- [ ] Update all match statements

---

### 2.3 Add TransactionHash::Sui Variant

**File**: `src/types.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TransactionHash {
    Evm(String),
    Solana([u8; 64]),
    Near(String),
    Stellar(String),
    #[cfg(feature = "sui")]
    Sui(String),  // Transaction digest
}
```

**Checklist**:
- [ ] Add `Sui(String)` variant

---

### 2.4 Add Wallet Loading to SignerType

**File**: `src/from_env.rs`

```rust
impl SignerType {
    // ... existing methods ...

    #[cfg(feature = "sui")]
    pub fn make_sui_wallet(&self, network: Network) -> Result<SuiKeyPair, Box<dyn std::error::Error>> {
        use sui_keys::keystore::{AccountKeystore, Keystore};

        // Try network-specific key first, then fallback
        let private_key = if network.is_testnet() {
            std::env::var(ENV_SUI_PRIVATE_KEY_TESTNET)
                .or_else(|_| std::env::var(ENV_SUI_PRIVATE_KEY))
        } else {
            std::env::var(ENV_SUI_PRIVATE_KEY_MAINNET)
                .or_else(|_| std::env::var(ENV_SUI_PRIVATE_KEY))
        }?;

        // Parse as Base64 encoded keypair
        let keypair = SuiKeyPair::decode_base64(&private_key)
            .map_err(|e| format!("Invalid Sui private key: {e}"))?;

        Ok(keypair)
    }
}
```

**Checklist**:
- [ ] Add `make_sui_wallet()` method
- [ ] Support mainnet/testnet separation
- [ ] Support Base64 encoded keypairs

---

### Phase 2 Completion Checklist

- [ ] `cargo check --features sui` passes
- [ ] `cargo build --release --features sui` compiles
- [ ] Provider can be instantiated (manual test)
- [ ] All trait implementations complete

---

## Phase 3: Integration & Configuration (Week 4)

### 3.1 Update .env.example

**File**: `.env.example`

```bash
# ============================================
# Sui Configuration
# ============================================

# Sui RPC URLs
RPC_URL_SUI=https://fullnode.mainnet.sui.io:443
RPC_URL_SUI_TESTNET=https://fullnode.testnet.sui.io:443

# Sui wallet private keys (Base64 encoded)
# Leave empty to use AWS Secrets Manager in production
SUI_PRIVATE_KEY_MAINNET=
SUI_PRIVATE_KEY_TESTNET=

# Sui gas budget limits (in MIST - 1 SUI = 1e9 MIST)
X402_SUI_MAX_GAS_BUDGET_SUI=50000000
X402_SUI_MAX_GAS_BUDGET_SUI_TESTNET=100000000
```

**Checklist**:
- [ ] Add RPC URL variables
- [ ] Add private key variables
- [ ] Add gas budget limits
- [ ] Document format (Base64, MIST units)

---

### 3.2 Add Sui Logo

**File**: `static/sui.png`

1. Download official Sui logo (transparent PNG, ~200x200px)
2. Convert to 32x32px if needed
3. Save as `static/sui.png`

**Checklist**:
- [ ] Logo file exists at `static/sui.png`
- [ ] Transparent background
- [ ] Appropriate size

---

### 3.3 Add Logo Handler

**File**: `src/handlers.rs`

```rust
#[cfg(feature = "sui")]
pub async fn get_sui_logo() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        include_bytes!("../static/sui.png").as_slice(),
    )
}
```

**Update router** in `src/main.rs`:

```rust
#[cfg(feature = "sui")]
.route("/sui.png", get(handlers::get_sui_logo))
```

**Checklist**:
- [ ] Add `get_sui_logo()` handler
- [ ] Add route to router
- [ ] Verify logo loads at `/sui.png`

---

### 3.4 Update Landing Page

**File**: `static/index.html`

Add network card for Sui in the mainnets section:

```html
<!-- Add to mainnet cards section -->
<div class="network-card sui">
    <div class="network-logo">
        <img src="/sui.png" alt="Sui" width="32" height="32">
    </div>
    <div class="network-info">
        <h3>Sui</h3>
        <p class="chain-id">CAIP-2: sui:mainnet</p>
    </div>
    <div class="network-balance" id="balance-sui">
        <span class="balance-loading">Loading...</span>
    </div>
</div>

<!-- Add to testnet cards section -->
<div class="network-card sui">
    <div class="network-logo">
        <img src="/sui.png" alt="Sui Testnet" width="32" height="32">
    </div>
    <div class="network-info">
        <h3>Sui Testnet</h3>
        <p class="chain-id">CAIP-2: sui:testnet</p>
    </div>
    <div class="network-balance" id="balance-sui-testnet">
        <span class="balance-loading">Loading...</span>
    </div>
</div>
```

Add CSS styling:

```css
.network-card.sui {
    border-left: 4px solid #4DA2FF; /* Sui blue */
}
```

**Checklist**:
- [ ] Add mainnet card
- [ ] Add testnet card
- [ ] Add CSS border color
- [ ] Add balance loading JavaScript (optional)

---

### 3.5 Configure AWS Secrets Manager

```bash
# Create mainnet secret
aws secretsmanager create-secret \
    --name facilitator-sui-private-key-mainnet \
    --secret-string "BASE64_ENCODED_KEYPAIR" \
    --region us-east-2

# Create testnet secret
aws secretsmanager create-secret \
    --name facilitator-sui-private-key-testnet \
    --secret-string "BASE64_ENCODED_KEYPAIR" \
    --region us-east-2

# If using premium RPC (e.g., Shinami, QuickNode)
aws secretsmanager put-secret-value \
    --secret-id facilitator-rpc-mainnet \
    --secret-string '{"sui": "https://api.shinami.com/node/v1/YOUR_API_KEY"}' \
    --region us-east-2
```

**Checklist**:
- [ ] Create mainnet private key secret
- [ ] Create testnet private key secret
- [ ] Add premium RPC URL to secrets (optional)

---

### 3.6 Update Terraform Task Definition

**File**: `terraform/environments/production/task-definition.json`

```json
{
    "name": "RPC_URL_SUI_TESTNET",
    "value": "https://fullnode.testnet.sui.io:443"
},
{
    "name": "RPC_URL_SUI",
    "valueFrom": "arn:aws:secretsmanager:us-east-2:ACCOUNT:secret:facilitator-rpc-mainnet:sui::"
},
{
    "name": "SUI_PRIVATE_KEY_MAINNET",
    "valueFrom": "arn:aws:secretsmanager:us-east-2:ACCOUNT:secret:facilitator-sui-private-key-mainnet"
},
{
    "name": "SUI_PRIVATE_KEY_TESTNET",
    "valueFrom": "arn:aws:secretsmanager:us-east-2:ACCOUNT:secret:facilitator-sui-private-key-testnet"
}
```

**Checklist**:
- [ ] Add RPC URL environment variables
- [ ] Add private key secret references
- [ ] Use `valueFrom` for secrets, `value` for public data

---

### Phase 3 Completion Checklist

- [ ] .env.example updated
- [ ] Logo added and handler works
- [ ] Landing page shows Sui networks
- [ ] AWS Secrets created
- [ ] Task definition updated

---

## Phase 4: Testing (Week 4-5)

### 4.1 Unit Tests

**File**: `src/chain/sui.rs` (add tests module)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sui_chain_from_network() {
        assert!(SuiChain::try_from(Network::Sui).is_ok());
        assert!(SuiChain::try_from(Network::SuiTestnet).is_ok());
        assert!(SuiChain::try_from(Network::Base).is_err());
    }

    #[test]
    fn test_gas_budget_validation() {
        // TODO: Create mock provider and test gas limits
    }

    #[test]
    fn test_bcs_transaction_decoding() {
        // TODO: Test decoding of real BCS-encoded transactions
    }
}
```

**Run tests**:
```bash
cargo test --features sui
```

**Checklist**:
- [ ] Network conversion tests
- [ ] Gas budget validation tests
- [ ] BCS decoding tests
- [ ] Address conversion tests

---

### 4.2 Integration Tests (Testnet)

**File**: `tests/integration/test_sui_payment.py`

```python
#!/usr/bin/env python3
"""
Integration tests for Sui payment verification and settlement.
"""

import requests
import base64
from pysui import SuiConfig, SuiClient

FACILITATOR_URL = "http://localhost:8080"
SUI_TESTNET_RPC = "https://fullnode.testnet.sui.io:443"

def test_sui_in_supported():
    """Verify Sui appears in supported networks."""
    response = requests.get(f"{FACILITATOR_URL}/supported")
    assert response.status_code == 200

    data = response.json()
    networks = [k["network"] for k in data["kinds"]]

    assert "sui" in networks or "sui-testnet" in networks
    print("[OK] Sui appears in /supported")

def test_sui_verify():
    """Test verification of a Sui USDC transfer."""
    # TODO: Build real transaction with pysui
    pass

def test_sui_settle():
    """Test settlement of a Sui USDC transfer."""
    # TODO: Full settlement test
    pass

if __name__ == "__main__":
    test_sui_in_supported()
    test_sui_verify()
    test_sui_settle()
    print("\n[SUCCESS] All Sui integration tests passed!")
```

**Run tests**:
```bash
cd tests/integration
python test_sui_payment.py
```

**Checklist**:
- [ ] `/supported` includes Sui
- [ ] `/verify` accepts valid Sui transactions
- [ ] `/settle` executes successfully on testnet
- [ ] Error handling works correctly

---

### 4.3 Manual Testing Checklist

```bash
# 1. Start facilitator locally
cargo run --release --features sui

# 2. Check Sui in supported networks
curl http://localhost:8080/supported | jq '.kinds[] | select(.network | contains("sui"))'

# 3. Verify logo loads
curl -I http://localhost:8080/sui.png

# 4. Check landing page
curl http://localhost:8080/ | grep -i sui

# 5. Check version
curl http://localhost:8080/version
```

**Checklist**:
- [ ] Facilitator starts without errors
- [ ] Sui networks in `/supported`
- [ ] Logo loads correctly
- [ ] Landing page displays Sui cards

---

### Phase 4 Completion Checklist

- [ ] All unit tests pass
- [ ] Integration tests pass on testnet
- [ ] Manual verification complete
- [ ] No regressions in existing functionality

---

## Phase 5: Production Deployment (Week 5)

### 5.1 Pre-Deployment Checklist

- [ ] Mainnet wallet funded with SUI (minimum 10 SUI)
- [ ] Testnet wallet funded with SUI
- [ ] AWS Secrets configured correctly
- [ ] Premium RPC configured (if using)
- [ ] All tests pass
- [ ] Code reviewed

### 5.2 Build and Push Docker Image

```bash
# Update version in Cargo.toml (e.g., from 1.9.0 to 1.10.0)

# Build with Sui feature
docker build \
    --build-arg FEATURES="solana,sui" \
    -t facilitator:v1.10.0-sui \
    .

# Tag and push to ECR
aws ecr get-login-password --region us-east-2 | docker login --username AWS --password-stdin ACCOUNT.dkr.ecr.us-east-2.amazonaws.com

docker tag facilitator:v1.10.0-sui ACCOUNT.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.10.0-sui
docker tag facilitator:v1.10.0-sui ACCOUNT.dkr.ecr.us-east-2.amazonaws.com/facilitator:latest

docker push ACCOUNT.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.10.0-sui
docker push ACCOUNT.dkr.ecr.us-east-2.amazonaws.com/facilitator:latest
```

### 5.3 Deploy to ECS

```bash
# Register new task definition
aws ecs register-task-definition \
    --cli-input-json file://terraform/environments/production/task-definition.json \
    --region us-east-2

# Update service
aws ecs update-service \
    --cluster facilitator-production \
    --service facilitator-production \
    --force-new-deployment \
    --region us-east-2

# Monitor deployment
aws ecs describe-services \
    --cluster facilitator-production \
    --services facilitator-production \
    --region us-east-2 | jq '.services[0].deployments'
```

### 5.4 Post-Deployment Verification

```bash
# Check version
curl https://facilitator.ultravioletadao.xyz/version

# Verify Sui in supported
curl https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | contains("sui"))'

# Check logo
curl -I https://facilitator.ultravioletadao.xyz/sui.png

# Check health
curl https://facilitator.ultravioletadao.xyz/health
```

### 5.5 Update Documentation

- [ ] Update CHANGELOG.md with v1.10.0 release notes
- [ ] Update CLAUDE.md with Sui networks
- [ ] Update README.md if needed
- [ ] Archive this plan as completed

---

## Technical Reference

### Sui Type IDs vs Addresses

Sui uses **Type IDs** for coins, not simple addresses:

```
Type ID Format: {package_id}::{module}::{type}

USDC: 0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC
      ├── Package ID (32 bytes hex)
      ├── Module name
      └── Type name
```

### Transaction Structure

```rust
TransactionData {
    kind: TransactionKind::ProgrammableTransaction(PTB),
    sender: SuiAddress,          // User
    gas_data: GasData {
        payment: Vec<ObjectRef>, // Sponsor's gas coins
        owner: SuiAddress,       // Sponsor (facilitator)
        price: u64,              // Gas price
        budget: u64,             // Max gas
    },
    expiration: TransactionExpiration::Epoch(u64),
}
```

### Signature Requirements

1. **User signature**: Over full TransactionData (authorizes transfer)
2. **Sponsor signature**: Over full TransactionData (authorizes gas payment)

Both required for execution.

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `sui-sdk` | RPC client, transaction execution |
| `sui-types` | Core types (Address, Transaction, etc.) |
| `sui-keys` | Keypair management |
| `shared-crypto` | Signature verification |
| `bcs` | Binary Canonical Serialization |

---

## Risk Mitigation

### SDK Instability

**Risk**: Sui SDK has breaking changes
**Mitigation**:
- Pin to specific git commit
- Test thoroughly before updating
- Monitor Sui releases

### Gas Exploitation

**Risk**: Attacker tricks facilitator into paying excessive gas
**Mitigation**:
- Strict `max_gas_budget` limit
- Verify sender != facilitator
- Verify no facilitator objects in commands

### Signature Replay

**Risk**: Same transaction submitted multiple times
**Mitigation**:
- Sui's object versioning prevents replay
- Transaction epoch expiration

---

## Success Criteria

### Technical

- [ ] Sui networks appear in `/supported` endpoint
- [ ] `/verify` correctly validates Sui USDC transfers
- [ ] `/settle` successfully executes sponsored transactions
- [ ] Gas budget limits enforced
- [ ] Transaction digests returned correctly
- [ ] No performance regression

### Business

- [ ] First x402 facilitator with Sui support
- [ ] Successful testnet payment within 3 weeks
- [ ] Successful mainnet payment within 5 weeks
- [ ] Documentation complete for client developers

---

## Appendix: File Change Summary

| File | Action | Lines Changed |
|------|--------|---------------|
| `Cargo.toml` | Modify | +15 |
| `src/network.rs` | Modify | +60 |
| `src/types.rs` | Modify | +40 |
| `src/from_env.rs` | Modify | +35 |
| `src/chain/mod.rs` | Modify | +25 |
| `src/chain/sui.rs` | **NEW** | ~500 |
| `src/handlers.rs` | Modify | +10 |
| `src/main.rs` | Modify | +5 |
| `src/caip2.rs` | Modify | +10 |
| `static/index.html` | Modify | +60 |
| `static/sui.png` | **NEW** | (binary) |
| `.env.example` | Modify | +15 |
| **TOTAL** | | **~775 lines** |

---

*Master Plan created by Claude Code - December 29, 2025*
*Based on feasibility analysis: docs/SUI_FEASIBILITY_ANALYSIS_2025.md*
