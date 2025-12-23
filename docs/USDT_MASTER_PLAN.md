# USDT Master Plan: Integration on Arbitrum, Celo, and Optimism

**Date:** December 21, 2024
**Version:** 1.0
**Author:** AI Agent
**Status:** Draft for Review

## Executive Summary

This document provides a comprehensive implementation plan for adding USDT (Tether USD with EIP-3009 support via the new USDT0 upgrade) to the x402-rs payment facilitator on **3 networks**: Arbitrum, Celo, and Optimism.

### Key Facts
- **Token Type:** USDT (Tether USD)
- **Contract Standard:** EIP-3009 `transferWithAuthorization`
- **Decimals:** 6 (all networks)
- **Networks:** Arbitrum mainnet, Celo mainnet, Optimism mainnet
- **Contract Addresses:**
  - Arbitrum: `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` (name: "USD₮0")
  - Celo: `0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e` (name: "Tether USD")
  - Optimism: `0x01bff41798a0bcf287b996046ca68b395dbc1071` (name: "USD₮0") **[NEW ADDRESS - verify this is correct!]**

### Implementation Scope
- **Estimated Duration:** 4-6 hours
- **Complexity:** Medium (follows existing EURC/AUSD/PYUSD pattern)
- **Files Modified:** 4 core files (types.rs, network.rs, index.html, CHANGELOG.md)
- **Testing Required:** Local verification + integration tests
- **Deployment:** Standard Docker build → ECR → ECS update

### Risk Assessment
- **Low Risk:** Well-established pattern, no breaking changes
- **Medium Risk:** USDT0 is new (recently upgraded from legacy USDT), requires careful EIP-712 domain verification
- **Mitigation:** Comprehensive pre-deployment testing with all 3 networks

---

## Table of Contents

1. [Pre-Implementation Research](#1-pre-implementation-research)
2. [Backend Implementation](#2-backend-implementation)
3. [Frontend Implementation](#3-frontend-implementation)
4. [Testing Checklist](#4-testing-checklist)
5. [Deployment Steps](#5-deployment-steps)
6. [Verification Plan](#6-verification-plan)
7. [Rollback Plan](#7-rollback-plan)
8. [Appendix](#appendix)

---

## 1. Pre-Implementation Research

### 1.1 EIP-712 Domain Verification

**CRITICAL TASK:** Verify the exact EIP-712 domain parameters for USDT0 on each network before implementation.

#### Required Information per Network

| Network | Contract Address | Chain ID | Name Field | Version Field | Status |
|---------|-----------------|----------|------------|---------------|--------|
| Arbitrum | `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` | 42161 | ? | ? | ❌ VERIFY |
| Celo | `0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e` | 42220 | ? | ? | ❌ VERIFY |
| Optimism | `0x01bff41798a0bcf287b996046ca68b395dbc1071` | 10 | ? | ? | ❌ VERIFY |

#### Verification Method

Use the following `cast` commands to query EIP-712 metadata:

```bash
# Arbitrum (RPC: your-arbitrum-rpc-url)
cast call 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 "name()" --rpc-url <ARBITRUM_RPC>
cast call 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 "version()" --rpc-url <ARBITRUM_RPC>
cast call 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 "decimals()" --rpc-url <ARBITRUM_RPC>

# Celo
cast call 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e "name()" --rpc-url <CELO_RPC>
cast call 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e "version()" --rpc-url <CELO_RPC>
cast call 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e "decimals()" --rpc-url <CELO_RPC>

# Optimism
cast call 0x01bff41798a0bcf287b996046ca68b395dbc1071 "name()" --rpc-url <OPTIMISM_RPC>
cast call 0x01bff41798a0bcf287b996046ca68b395dbc1071 "version()" --rpc-url <OPTIMISM_RPC>
cast call 0x01bff41798a0bcf287b996046ca68b395dbc1071 "decimals()" --rpc-url <OPTIMISM_RPC>
```

#### Expected Values (Based on Research)

From your research notes:
- Arbitrum: name = "USD₮0", decimals = 6
- Celo: name = "Tether USD", decimals = 6
- Optimism: name = "USD₮0", decimals = 6 (NEW ADDRESS - VERIFY!)

**IMPORTANT:** The EIP-712 `version` field is critical for signature verification. Common values are "1" or "2". This MUST be verified before implementation.

#### Alternative Verification Method

If `version()` is not exposed (some contracts use internal variables), check Etherscan/block explorer:
1. Navigate to contract on block explorer
2. Look for "Read Contract" tab
3. Find `DOMAIN_SEPARATOR()` function
4. Decode the domain separator to extract name/version/chainId

Or use Python script:
```python
from eth_abi import decode
from web3 import Web3

# Example for Arbitrum
w3 = Web3(Web3.HTTPProvider('https://arb1.arbitrum.io/rpc'))
contract = w3.eth.contract(
    address='0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9',
    abi=[{"inputs":[],"name":"name","outputs":[{"internalType":"string","name":"","type":"string"}],"stateMutability":"view","type":"function"}]
)
name = contract.functions.name().call()
print(f"Name: {name}")
```

### 1.2 Contract Verification

Verify the contracts support EIP-3009:

```bash
# Check for transferWithAuthorization function signature
cast sig "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)"
# Expected: 0xe3ee160e

# Verify function exists on contract
cast call <CONTRACT_ADDRESS> "0xe3ee160e000000000000000000000000..." --rpc-url <RPC_URL>
```

### 1.3 Existing USDT Support Check

Verify that we do NOT already support USDT (to avoid duplication):

```bash
cd /mnt/z/ultravioleta/dao/x402-rs
grep -r "USDT\|Tether" src/ docs/ static/ --exclude-dir=.git
```

Expected: Should only find references in this plan or research notes, NOT in production code.

---

## 2. Backend Implementation

### 2.1 Update `src/types.rs`

**File:** `/mnt/z/ultravioleta/dao/x402-rs/src/types.rs`

**Location:** Line ~133-149 (TokenType enum)

**Change:** Add USDT variant to the `TokenType` enum

```rust
// BEFORE (current)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TokenType {
    #[default]
    #[serde(rename = "usdc")]
    Usdc,
    #[serde(rename = "eurc")]
    Eurc,
    #[serde(rename = "ausd")]
    Ausd,
    #[serde(rename = "pyusd")]
    Pyusd,
}

// AFTER (with USDT)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TokenType {
    #[default]
    #[serde(rename = "usdc")]
    Usdc,
    #[serde(rename = "eurc")]
    Eurc,
    #[serde(rename = "ausd")]
    Ausd,
    #[serde(rename = "pyusd")]
    Pyusd,
    /// Tether USD (USDT0 with EIP-3009 support) - 6 decimals
    #[serde(rename = "usdt")]
    Usdt,
}
```

**Location:** Line ~156-163 (decimals method)

```rust
// Add to decimals() method
pub const fn decimals(&self) -> u8 {
    match self {
        TokenType::Usdc => 6,
        TokenType::Eurc => 6,
        TokenType::Ausd => 6,
        TokenType::Pyusd => 6,
        TokenType::Usdt => 6,  // ADD THIS LINE
    }
}
```

**Location:** Line ~166-173 (symbol method)

```rust
pub const fn symbol(&self) -> &'static str {
    match self {
        TokenType::Usdc => "USDC",
        TokenType::Eurc => "EURC",
        TokenType::Ausd => "AUSD",
        TokenType::Pyusd => "PYUSD",
        TokenType::Usdt => "USDT",  // ADD THIS LINE
    }
}
```

**Location:** Line ~176-184 (display_name method)

```rust
pub const fn display_name(&self) -> &'static str {
    match self {
        TokenType::Usdc => "USD Coin",
        TokenType::Eurc => "Euro Coin",
        TokenType::Ausd => "Agora USD",
        TokenType::Pyusd => "PayPal USD",
        TokenType::Usdt => "Tether USD",  // ADD THIS LINE
    }
}
```

**Location:** Line ~187-195 (currency_symbol method)

```rust
pub const fn currency_symbol(&self) -> &'static str {
    match self {
        TokenType::Usdc => "$",
        TokenType::Eurc => "EUR",
        TokenType::Ausd => "$",
        TokenType::Pyusd => "$",
        TokenType::Usdt => "$",  // ADD THIS LINE
    }
}
```

**Location:** Line ~198-207 (is_fiat_backed method)

```rust
pub const fn is_fiat_backed(&self) -> bool {
    match self {
        TokenType::Usdc => true,
        TokenType::Eurc => true,
        TokenType::Ausd => true,
        TokenType::Pyusd => true,
        TokenType::Usdt => true,  // ADD THIS LINE
    }
}
```

**Location:** Line ~210-218 (all method)

```rust
pub const fn all() -> &'static [TokenType] {
    &[
        TokenType::Usdc,
        TokenType::Eurc,
        TokenType::Ausd,
        TokenType::Pyusd,
        TokenType::Usdt,  // ADD THIS LINE
    ]
}
```

**Location:** Line ~220-246 (eip712_name and eip712_version methods)

```rust
pub const fn eip712_name(&self) -> &'static str {
    match self {
        TokenType::Usdc => "USD Coin",
        TokenType::Eurc => "Euro Coin",
        TokenType::Ausd => "AUSD",
        TokenType::Pyusd => "PayPal USD",
        // ADD THIS - VERIFY ACTUAL VALUES FROM STEP 1.1
        TokenType::Usdt => "USD₮0",  // Or "Tether USD" - CHECK CONTRACT!
    }
}

pub const fn eip712_version(&self) -> &'static str {
    match self {
        TokenType::Usdc => "2",
        TokenType::Eurc => "2",
        TokenType::Ausd => "1",
        TokenType::Pyusd => "1",
        // ADD THIS - VERIFY ACTUAL VALUE FROM STEP 1.1
        TokenType::Usdt => "1",  // VERIFY THIS!
    }
}
```

**⚠️ CRITICAL:** The `eip712_name()` value MUST match EXACTLY what the contract returns, or signatures will fail. Use results from Step 1.1.

**Location:** Line ~267-277 (FromStr implementation)

```rust
impl FromStr for TokenType {
    type Err = TokenTypeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "usdc" => Ok(TokenType::Usdc),
            "eurc" => Ok(TokenType::Eurc),
            "ausd" => Ok(TokenType::Ausd),
            "pyusd" => Ok(TokenType::Pyusd),
            "usdt" => Ok(TokenType::Usdt),  // ADD THIS LINE
            _ => Err(TokenTypeParseError(s.to_string())),
        }
    }
}
```

**Location:** Line ~281-283 (TokenTypeParseError error message)

```rust
#[derive(Debug, Clone, thiserror::Error)]
#[error("Unknown token type: {0}. Supported: usdc, eurc, ausd, pyusd, usdt")]  // UPDATE THIS
pub struct TokenTypeParseError(pub String);
```

---

### 2.2 Update `src/network.rs`

**File:** `/mnt/z/ultravioleta/dao/x402-rs/src/network.rs`

**Location:** After line 1197 (after PYUSD deployments)

Add USDT deployment constants:

```rust
// ============================================================================
// USDT (Tether USD) Deployments - USDT0 with EIP-3009 Support
// ============================================================================

/// USDT deployment on Arbitrum mainnet.
/// NOTE: Uses new USDT0 contract with EIP-3009 support (not legacy USDT).
static USDT_ARBITRUM: Lazy<USDTDeployment> = Lazy::new(|| {
    USDTDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9").into(),
            network: Network::Arbitrum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD₮0".into(),  // VERIFY THIS IN STEP 1.1!
            version: "1".into(),   // VERIFY THIS IN STEP 1.1!
        }),
    })
});

/// USDT deployment on Celo mainnet.
static USDT_CELO: Lazy<USDTDeployment> = Lazy::new(|| {
    USDTDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e").into(),
            network: Network::Celo,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "Tether USD".into(),  // VERIFY THIS IN STEP 1.1!
            version: "1".into(),         // VERIFY THIS IN STEP 1.1!
        }),
    })
});

/// USDT deployment on Optimism mainnet.
/// NOTE: This is a NEW contract address (not the legacy bridged USDT).
static USDT_OPTIMISM: Lazy<USDTDeployment> = Lazy::new(|| {
    USDTDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x01bff41798a0bcf287b996046ca68b395dbc1071").into(),
            network: Network::Optimism,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD₮0".into(),  // VERIFY THIS IN STEP 1.1!
            version: "1".into(),   // VERIFY THIS IN STEP 1.1!
        }),
    })
});

/// A known USDT (Tether USD) deployment as a wrapper around [`TokenDeployment`].
#[derive(Clone, Debug)]
pub struct USDTDeployment(pub TokenDeployment);

impl Deref for USDTDeployment {
    type Target = TokenDeployment;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&USDTDeployment> for TokenDeployment {
    fn from(deployment: &USDTDeployment) -> Self {
        deployment.0.clone()
    }
}

impl USDTDeployment {
    /// Return the known USDT deployment for the given network.
    ///
    /// Returns `None` if USDT is not deployed on the specified network.
    /// Note: USDT0 (with EIP-3009) is currently only on Arbitrum, Celo, and Optimism.
    pub fn by_network<N: Borrow<Network>>(network: N) -> Option<&'static USDTDeployment> {
        match network.borrow() {
            Network::Arbitrum => Some(&USDT_ARBITRUM),
            Network::Celo => Some(&USDT_CELO),
            Network::Optimism => Some(&USDT_OPTIMISM),
            _ => None,
        }
    }

    /// Return all networks where USDT (with EIP-3009) is deployed.
    pub fn supported_networks() -> &'static [Network] {
        &[Network::Arbitrum, Network::Celo, Network::Optimism]
    }
}
```

**Location:** Line ~1216-1223 (get_token_deployment function)

Update the `get_token_deployment` function to include USDT:

```rust
pub fn get_token_deployment(network: Network, token_type: TokenType) -> Option<TokenDeployment> {
    match token_type {
        TokenType::Usdc => Some(USDCDeployment::by_network(network).0.clone()),
        TokenType::Eurc => EURCDeployment::by_network(network).map(|d| d.0.clone()),
        TokenType::Ausd => AUSDDeployment::by_network(network).map(|d| d.0.clone()),
        TokenType::Pyusd => PYUSDDeployment::by_network(network).map(|d| d.0.clone()),
        TokenType::Usdt => USDTDeployment::by_network(network).map(|d| d.0.clone()),  // ADD THIS LINE
    }
}
```

**Location:** Line ~1267-1273 (supported_networks_for_token function)

```rust
pub fn supported_networks_for_token(token_type: TokenType) -> Vec<Network> {
    match token_type {
        TokenType::Usdc => Network::variants().to_vec(),
        TokenType::Eurc => EURCDeployment::supported_networks().to_vec(),
        TokenType::Ausd => AUSDDeployment::supported_networks().to_vec(),
        TokenType::Pyusd => PYUSDDeployment::supported_networks().to_vec(),
        TokenType::Usdt => USDTDeployment::supported_networks().to_vec(),  // ADD THIS LINE
    }
}
```

**Location:** In the `use` statement at top of file (around line 6)

Add import for the new deployment struct:

```rust
use crate::network::{
    get_token_deployment, supported_tokens_for_network, AUSDDeployment, EURCDeployment, Network,
    PYUSDDeployment, USDCDeployment, USDTDeployment,  // ADD USDTDeployment
};
```

---

### 2.3 Update `src/chain/evm.rs`

**File:** `/mnt/z/ultravioleta/dao/x402-rs/src/chain/evm.rs`

**Location:** Around line 48-51 (use statement)

Add `USDTDeployment` to the imports:

```rust
use crate::network::{
    get_token_deployment, supported_tokens_for_network, AUSDDeployment, EURCDeployment, Network,
    PYUSDDeployment, USDCDeployment, USDTDeployment,  // ADD THIS
};
```

**Location:** Line ~1115-1161 (find_known_eip712_metadata function)

Add USDT check to the existing lookup function:

```rust
fn find_known_eip712_metadata(
    network: Network,
    asset_address: &Address,
) -> Option<(String, String)> {
    let asset_mixed: MixedAddress = (*asset_address).into();

    // Check USDC (always available on all networks)
    let usdc = USDCDeployment::by_network(network);
    if usdc.address() == asset_mixed {
        if let Some(eip712) = &usdc.eip712 {
            return Some((eip712.name.clone(), eip712.version.clone()));
        }
    }

    // Check EURC
    if let Some(eurc) = EURCDeployment::by_network(network) {
        if eurc.address() == asset_mixed {
            if let Some(eip712) = &eurc.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check AUSD
    if let Some(ausd) = AUSDDeployment::by_network(network) {
        if ausd.address() == asset_mixed {
            if let Some(eip712) = &ausd.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // Check PYUSD
    if let Some(pyusd) = PYUSDDeployment::by_network(network) {
        if pyusd.address() == asset_mixed {
            if let Some(eip712) = &pyusd.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    // ADD THIS BLOCK - Check USDT
    if let Some(usdt) = USDTDeployment::by_network(network) {
        if usdt.address() == asset_mixed {
            if let Some(eip712) = &usdt.eip712 {
                return Some((eip712.name.clone(), eip712.version.clone()));
            }
        }
    }

    None
}
```

**Note:** No changes needed to signature handling - USDT0 uses standard compact signature format (not v,r,s like PYUSD).

---

### 2.4 Summary of Backend Changes

| File | Lines Added | Lines Modified | Function |
|------|-------------|----------------|----------|
| `src/types.rs` | ~10 | ~40 | Add USDT enum variant + metadata |
| `src/network.rs` | ~100 | ~10 | Add USDT deployment constants + lookup |
| `src/chain/evm.rs` | ~10 | ~2 | Add USDT EIP-712 metadata lookup |
| **Total** | **~120** | **~52** | |

---

## 3. Frontend Implementation

### 3.1 Update `static/index.html`

**File:** `/mnt/z/ultravioleta/dao/x402-rs/static/index.html`

**Location:** Line ~2182-2213 (TOKEN_SUPPORT object)

Add USDT to the networks that support it:

```javascript
// Token support configuration per network (based on EIP-3009 verification)
// See docs/STABLECOIN_EXPANSION_PLAN.md for verification details
const TOKEN_SUPPORT = {
    // Mainnets
    'ethereum-mainnet': ['usdc', 'eurc', 'ausd', 'pyusd'],
    'base-mainnet': ['usdc', 'eurc'],
    'avalanche-mainnet': ['usdc', 'eurc', 'ausd'],
    'polygon-mainnet': ['usdc', 'ausd'],
    'arbitrum-mainnet': ['usdc', 'ausd', 'usdt'],  // ADD 'usdt'
    'optimism-mainnet': ['usdc', 'usdt'],  // ADD 'usdt'
    'celo-mainnet': ['usdc', 'usdt'],  // ADD 'usdt'
    'hyperevm-mainnet': ['usdc'],
    'unichain-mainnet': ['usdc'],
    'monad-mainnet': ['usdc'],
    // Testnets (no USDT testnets for now)
    'base-sepolia': ['usdc', 'eurc'],
    'avalanche-fuji': ['usdc'],
    'polygon-amoy': ['usdc'],
    'arbitrum-sepolia': ['usdc'],
    'optimism-sepolia': ['usdc'],
    'celo-sepolia': ['usdc'],
    'hyperevm-testnet': ['usdc'],
    'ethereum-sepolia': ['usdc'],
    // Non-EVM chains
    'solana-mainnet': ['usdc'],
    'solana-devnet': ['usdc'],
    'fogo-mainnet': ['usdc'],
    'fogo-testnet': ['usdc'],
    'near-mainnet': ['usdc'],
    'near-testnet': ['usdc'],
    'stellar-mainnet': ['usdc'],
    'stellar-testnet': ['usdc'],
};
```

**Location:** Line ~2215-2220 (TOKEN_INFO object)

Add USDT metadata:

```javascript
const TOKEN_INFO = {
    'usdc': { name: 'USDC', decimals: 6 },
    'eurc': { name: 'EURC', decimals: 6 },
    'ausd': { name: 'AUSD', decimals: 6 },
    'pyusd': { name: 'PYUSD', decimals: 6 },
    'usdt': { name: 'USDT', decimals: 6 },  // ADD THIS LINE
};
```

**Location:** In the `<style>` section (around line 800-900)

Add USDT-specific token pill styling:

```css
/* Existing token pill styles */
.token-pill.usdc { background: #2775CA; color: white; }
.token-pill.eurc { background: #4E9F3D; color: white; }
.token-pill.ausd { background: #FF6B6B; color: white; }
.token-pill.pyusd { background: #0070BA; color: white; }

/* ADD THIS */
.token-pill.usdt { background: #50AF95; color: white; }  /* Tether green */
```

### 3.2 Frontend Summary

| Section | Change | Impact |
|---------|--------|--------|
| TOKEN_SUPPORT | Add 'usdt' to 3 networks | Frontend displays USDT badges |
| TOKEN_INFO | Add USDT metadata | Tooltips show correct info |
| CSS | Add .token-pill.usdt style | USDT pill has Tether green color |

---

## 4. Testing Checklist

### 4.1 Pre-Deployment Testing (Local)

- [ ] **Build Test:** `cargo build --release` succeeds without errors
- [ ] **Clippy:** `cargo clippy --all-targets --all-features` passes
- [ ] **Format:** `cargo fmt --all -- --check` passes
- [ ] **Unit Tests:** `cargo test` all tests pass
- [ ] **Type Tests:** Verify TokenType serde works
  ```bash
  # In cargo test output, check:
  # test types::tests::test_token_type_serde_serialization ... ok
  # test types::tests::test_token_type_from_str ... ok
  ```

### 4.2 Integration Testing (Local Facilitator)

Start local facilitator with USDT-enabled RPC URLs:

```bash
# Set environment variables
export RPC_URL_ARBITRUM=https://arb1.arbitrum.io/rpc
export RPC_URL_CELO=https://forno.celo.org
export RPC_URL_OPTIMISM=https://mainnet.optimism.io

# Run facilitator
RUST_LOG=debug cargo run --release
```

**Test 1: `/supported` Endpoint**

```bash
curl http://localhost:8080/supported | jq '.kinds[] | select(.network == "arbitrum") | .extra.tokens'
# Expected output should include USDT:
# [
#   {"token": "usdc", "address": "0xaf88d065e77c8cC2239327C5EDb3A432268e5831", "decimals": 6},
#   {"token": "ausd", "address": "0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a", "decimals": 6},
#   {"token": "usdt", "address": "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9", "decimals": 6}
# ]
```

**Test 2: Frontend Display**

```bash
# Open http://localhost:8080/ in browser
# Verify:
# - Arbitrum card shows USDT badge (green color)
# - Celo card shows USDT badge
# - Optimism card shows USDT badge
# - Other networks do NOT show USDT badge
```

**Test 3: Payment Verification (Arbitrum USDT)**

```bash
# Use test script (requires funded test wallet)
cd tests/integration
python test_usdt_payment.py --network arbitrum --token usdt
```

Expected result:
- Payment payload verified successfully
- Correct EIP-712 domain used
- Balance check passes

**Test 4: Payment Settlement (Celo USDT)**

```bash
python test_usdt_payment.py --network celo --token usdt --settle
```

Expected result:
- Transaction submitted successfully
- `transferWithAuthorization` called with correct signature
- Receipt returned with success status

### 4.3 Contract Verification Tests

For each network, verify the contract responds correctly:

```bash
# Test Arbitrum USDT contract
cast call 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 "name()" --rpc-url https://arb1.arbitrum.io/rpc
cast call 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 "decimals()" --rpc-url https://arb1.arbitrum.io/rpc
cast call 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 "DOMAIN_SEPARATOR()" --rpc-url https://arb1.arbitrum.io/rpc

# Test Celo USDT contract
cast call 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e "name()" --rpc-url https://forno.celo.org
cast call 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e "decimals()" --rpc-url https://forno.celo.org

# Test Optimism USDT contract
cast call 0x01bff41798a0bcf287b996046ca68b395dbc1071 "name()" --rpc-url https://mainnet.optimism.io
cast call 0x01bff41798a0bcf287b996046ca68b395dbc1071 "decimals()" --rpc-url https://mainnet.optimism.io
```

Expected: All calls return valid responses (name, decimals = 6).

---

## 5. Deployment Steps

### 5.1 Pre-Deployment Checklist

- [ ] All tests passing (section 4)
- [ ] EIP-712 metadata verified (section 1.1)
- [ ] CHANGELOG.md updated (section 6.5)
- [ ] Git commit created with clear message
- [ ] Docker build tested locally

### 5.2 Version Bump

Check current deployed version:

```bash
curl -s https://facilitator.ultravioletadao.xyz/version
# Example output: "1.9.4"
```

Bump version in `Cargo.toml`:

```toml
[package]
name = "x402-rs"
version = "1.10.7"  # Increment from deployed version
```

Update `Cargo.lock`:

```bash
cargo build --release
```

### 5.3 Build Docker Image

```bash
# Build locally first to catch errors
docker build -t facilitator-test .

# Test the Docker image
docker run -p 8080:8080 -e RPC_URL_ARBITRUM=https://arb1.arbitrum.io/rpc facilitator-test

# Verify it works
curl http://localhost:8080/supported | jq '.kinds[] | select(.network == "arbitrum")'
```

### 5.4 Push to ECR

```bash
# Use the build-and-push script
cd /mnt/z/ultravioleta/dao/x402-rs
./scripts/build-and-push.sh v1.10.7
```

This script:
1. Builds the Docker image with the version tag
2. Tags it for ECR
3. Pushes to AWS ECR repository
4. Outputs the image URI for deployment

### 5.5 Deploy to ECS

```bash
# Update ECS service with new image
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# Monitor deployment
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments'
```

Expected:
- New task starts with updated image
- Health checks pass
- Old task drains and terminates

### 5.6 Post-Deployment Verification

```bash
# Check version
curl -s https://facilitator.ultravioletadao.xyz/version
# Expected: "1.10.7"

# Verify USDT support
curl -s https://facilitator.ultravioletadao.xyz/supported | \
  jq '.kinds[] | select(.network == "arbitrum") | .extra.tokens[] | select(.token == "usdt")'
# Expected: {"token": "usdt", "address": "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9", "decimals": 6}

# Check health
curl -s https://facilitator.ultravioletadao.xyz/health
# Expected: {"status": "healthy"}

# Verify frontend
curl -s https://facilitator.ultravioletadao.xyz/ | grep -o "usdt" | wc -l
# Expected: >0 (USDT appears in HTML)
```

---

## 6. Verification Plan

### 6.1 Production Smoke Tests

Once deployed, run these manual verification tests:

**Test 1: Arbitrum USDT Payment**
- Network: Arbitrum mainnet
- Token: USDT
- Amount: 0.01 USDT (10000 units)
- Expected: Payment verifies and settles successfully

**Test 2: Celo USDT Payment**
- Network: Celo mainnet
- Token: USDT
- Amount: 0.01 USDT
- Expected: Payment verifies and settles successfully

**Test 3: Optimism USDT Payment**
- Network: Optimism mainnet
- Token: USDT
- Amount: 0.01 USDT
- Expected: Payment verifies and settles successfully

### 6.2 Monitoring

Check CloudWatch logs for errors:

```bash
aws logs tail /ecs/facilitator-production --follow --region us-east-2 | grep -i "usdt\|error"
```

Expected: No errors related to USDT token handling.

### 6.3 User Acceptance Testing

- [ ] Arbitrum card on landing page shows USDT badge
- [ ] Celo card shows USDT badge
- [ ] Optimism card shows USDT badge
- [ ] USDT badge is Tether green color (#50AF95)
- [ ] Tooltips show correct USDT info (6 decimals)
- [ ] `/supported` endpoint includes USDT on all 3 networks
- [ ] No other networks show USDT support

---

## 7. Rollback Plan

### 7.1 Immediate Rollback (Critical Issues)

If deployment causes critical failures:

```bash
# Revert to previous task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:<PREVIOUS_REVISION> \
  --force-new-deployment \
  --region us-east-2
```

To find previous revision:

```bash
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 \
  --query 'taskDefinition.revision'
```

### 7.2 Code Rollback

If Docker rollback is not sufficient:

```bash
# Revert git commit
cd /mnt/z/ultravioleta/dao/x402-rs
git log --oneline | head -5  # Find commit before USDT integration
git revert <COMMIT_HASH>

# Rebuild and redeploy
./scripts/build-and-push.sh v1.9.4-hotfix
aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2
```

### 7.3 Partial Rollback

If USDT works on some networks but not others, you can disable specific networks:

**Option 1:** Remove from frontend only (backend still supports it):
- Edit `static/index.html`
- Remove 'usdt' from failing network in TOKEN_SUPPORT
- Redeploy

**Option 2:** Remove from backend:
- Comment out USDT deployment for failing network in `src/network.rs`
- Rebuild and redeploy

---

## Appendix

### A. EIP-712 Domain Reference

Standard EIP-712 domain structure for EIP-3009 tokens:

```solidity
struct EIP712Domain {
    string name;       // Token name (e.g., "USD₮0" or "Tether USD")
    string version;    // Domain version (e.g., "1" or "2")
    uint256 chainId;   // Network chain ID
    address verifyingContract;  // Token contract address
}
```

### B. Contract Addresses Summary

| Network | Contract Address | Name | Version | Decimals |
|---------|-----------------|------|---------|----------|
| Arbitrum | `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` | USD₮0 | ? | 6 |
| Celo | `0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e` | Tether USD | ? | 6 |
| Optimism | `0x01bff41798a0bcf287b996046ca68b395dbc1071` | USD₮0 | ? | 6 |

**⚠️ Version field MUST be verified before implementation.**

### C. File Modification Summary

| File | Purpose | Estimated Time |
|------|---------|----------------|
| `src/types.rs` | Add USDT enum + metadata | 30 min |
| `src/network.rs` | Add USDT deployments | 45 min |
| `src/chain/evm.rs` | Add USDT EIP-712 lookup | 15 min |
| `static/index.html` | Frontend token support | 20 min |
| `docs/CHANGELOG.md` | Document changes | 20 min |
| **Testing** | Local + integration tests | 60 min |
| **Deployment** | Build + push + verify | 45 min |
| **Total** | | **~4 hours** |

### D. Testing Scripts

Create `tests/integration/test_usdt_payment.py`:

```python
#!/usr/bin/env python3
"""
Integration test for USDT payments on Arbitrum, Celo, and Optimism.
"""

import argparse
import requests
import json

NETWORKS = {
    'arbitrum': {
        'name': 'arbitrum',
        'chain_id': 42161,
        'usdt_address': '0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9',
    },
    'celo': {
        'name': 'celo',
        'chain_id': 42220,
        'usdt_address': '0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e',
    },
    'optimism': {
        'name': 'optimism',
        'chain_id': 10,
        'usdt_address': '0x01bff41798a0bcf287b996046ca68b395dbc1071',
    },
}

def test_supported_endpoint(base_url, network):
    """Test that /supported includes USDT for the network."""
    response = requests.get(f"{base_url}/supported")
    data = response.json()

    for kind in data['kinds']:
        if kind['network'] == network:
            tokens = kind.get('extra', {}).get('tokens', [])
            usdt_tokens = [t for t in tokens if t['token'] == 'usdt']

            if usdt_tokens:
                print(f"[OK] {network} supports USDT: {usdt_tokens[0]}")
                return True
            else:
                print(f"[FAIL] {network} does not list USDT in supported tokens")
                return False

    print(f"[FAIL] {network} not found in /supported response")
    return False

def main():
    parser = argparse.ArgumentParser(description='Test USDT payment integration')
    parser.add_argument('--network', choices=['arbitrum', 'celo', 'optimism'], required=True)
    parser.add_argument('--base-url', default='http://localhost:8080')
    args = parser.parse_args()

    network_config = NETWORKS[args.network]

    print(f"Testing USDT on {args.network}...")
    print(f"USDT address: {network_config['usdt_address']}")

    # Test /supported endpoint
    if not test_supported_endpoint(args.base_url, network_config['name']):
        exit(1)

    print("\n[SUCCESS] All tests passed!")

if __name__ == '__main__':
    main()
```

Make executable:
```bash
chmod +x tests/integration/test_usdt_payment.py
```

### E. CHANGELOG Entry Template

Add to `/mnt/z/ultravioleta/dao/x402-rs/docs/CHANGELOG.md`:

```markdown
## [1.10.7] - 2024-12-21

### Added - USDT Support on 3 Networks

This release adds support for USDT (Tether USD with EIP-3009 support) on Arbitrum, Celo, and Optimism.

#### New Token Support

| Token | Networks | Decimals | Description |
|-------|----------|----------|-------------|
| **USDT** | Arbitrum, Celo, Optimism | 6 | Tether USD (USDT0 with EIP-3009) |

#### Contract Addresses

```
USDT:
  Arbitrum:  0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9 (USD₮0)
  Celo:      0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e (Tether USD)
  Optimism:  0x01bff41798a0bcf287b996046ca68b395dbc1071 (USD₮0)
```

#### Changes

- **TokenType Enum**: Added `Usdt` variant to `src/types.rs`
- **Token Deployments**: Added USDT deployment constants in `src/network.rs`
- **EIP-712 Metadata**: Added USDT domain lookup in `src/chain/evm.rs`
- **Frontend**: Added USDT badges (Tether green #50AF95) to Arbitrum, Celo, and Optimism cards

#### Backward Compatibility

- No breaking changes
- USDC remains the default token
- Existing clients work without modification

#### Notes

- USDT0 is the new EIP-3009 compatible USDT contract (not legacy USDT)
- Only available on Arbitrum, Celo, and Optimism (as of this release)
- Uses standard compact signature format (not v,r,s variant)
```

---

## Summary

This master plan provides a complete roadmap for integrating USDT on 3 networks. Follow the steps in order:

1. **Research** (Section 1) - Verify EIP-712 metadata FIRST
2. **Backend** (Section 2) - Add USDT to Rust code
3. **Frontend** (Section 3) - Update HTML with USDT badges
4. **Testing** (Section 4) - Comprehensive local + integration tests
5. **Deployment** (Section 5) - Docker build → ECR → ECS
6. **Verification** (Section 6) - Production smoke tests
7. **Rollback** (Section 7) - Emergency procedures if needed

**Estimated Total Time:** 4-6 hours (including testing and deployment)

**Risk Level:** Low-Medium (well-established pattern, careful EIP-712 verification required)

---

**Next Steps:**
1. Review this plan with the team
2. Execute Section 1 (EIP-712 verification) to get exact domain metadata
3. Update this plan with verified EIP-712 values
4. Proceed with implementation following Sections 2-7

**Questions or Issues:**
- Contact: [Your team contact info]
- Slack channel: [Your channel]
- Documentation: `/mnt/z/ultravioleta/dao/x402-rs/docs/`
