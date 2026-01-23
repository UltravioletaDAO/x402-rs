# Superfluid x402 Integration Implementation Plan

**Document Version**: 2.1
**Created**: 2026-01-13
**Updated**: 2026-01-13
**Status**: Proposed (Revised after Agent Review)
**Author**: task-decomposition-expert + claude-opus-4-5
**Reviewed by**: task-decomposition-expert, security-auditor, aegis-rust-architect

---

## Executive Summary

**Objective**: Add Superfluid streaming payment capabilities to the x402-rs facilitator, enabling subscription-based access to protected resources via continuous payment streams.

**Key Features**:
1. **Multi-Network Support** - All networks where x402-rs AND Superfluid overlap (8 mainnets + 4 testnets)
2. **Multi-Token Support** - USDCx, UVDx (Ultravioleta DAO), and any registered Super Token
3. **Token Selection** - Client chooses which Super Token to wrap/stream

**Strategic Value for Ultravioleta DAO**:
1. **Revenue Model Innovation**: Enable subscription/streaming revenue for DAO-operated APIs
2. **Competitive Differentiation**: First Rust-based x402 facilitator with Superfluid support
3. **Network Effect**: Tap into Superfluid's growing ecosystem
4. **UVD Integration**: Native support for streaming UVDx to DAO members

---

### Two-Phase Implementation

| Phase | Smart Contracts? | What You Get | Effort |
|-------|------------------|--------------|--------|
| **Phase 1** | **NO** (Rust only) | Gasless Wrapping Service (wrap + transfer, manual stream) | 9-10 days |
| **Phase 2** | **YES** (SuperfluidEscrow.sol) | Full Escrow-Backed Streaming with trustless refunds | +12-15 days |

**Phase 1 Reality Check** (from agent review):
- User gets Super Tokens (USDCx/UVDx) gaslessly
- User must manually create streams via Superfluid Dashboard
- ACL-based streaming requires user to pre-grant permissions (defeats gasless)

**Phase 2 Delivers**: True automatic streaming with trustless refunds

---

**Complexity**: High (multi-network, multi-token)
**Phase 1 Effort**: 9-10 days (includes security fixes, staged deployment)
**Phase 2 Effort**: +12-15 days (includes professional security audit)

> **See**: `docs/SUPERFLUID_AGENT_REVIEW.md` for detailed security findings

---

## Network Support Matrix

### Networks with BOTH x402-rs AND Superfluid Support

| Network | Chain ID | x402-rs | Superfluid | Status |
|---------|----------|---------|------------|--------|
| **Ethereum** | 1 | YES | YES | Full Support |
| **Base** | 8453 | YES | YES | Full Support |
| **Polygon** | 137 | YES | YES | Full Support |
| **Optimism** | 10 | YES | YES | Full Support |
| **Arbitrum** | 42161 | YES | YES | Full Support |
| **Avalanche** | 43114 | YES | YES | Full Support + UVDx |
| **Celo** | 42220 | YES | YES | Full Support |
| **BSC** | 56 | YES | YES | Full Support |
| **Base Sepolia** | 84532 | YES | YES | Testnet |
| **Optimism Sepolia** | 11155420 | YES | YES | Testnet |
| **Avalanche Fuji** | 43113 | YES | YES | Testnet |
| **Ethereum Sepolia** | 11155111 | YES | YES | Testnet |

### Networks WITHOUT Superfluid Support
- HyperEVM, Sei, Unichain, Monad, XDC, XRPL-EVM, Polygon Amoy, Celo Sepolia, Arbitrum Sepolia
- Non-EVM: Solana, NEAR, Stellar, Fogo, Algorand, Sui

---

## Superfluid Contract Addresses

### CFAv1Forwarder (SAME on most networks)
```
Mainnet: 0xcfA132E353cB4E398080B9700609bb008eceB125
Fuji:    0x2CDd45c5182602a36d391F7F16DD9f8386C3bD8D
```

### Host Contracts (per network)

| Network | Chain ID | Host Address |
|---------|----------|--------------|
| Ethereum | 1 | `0x4E583d9390082B65Bef884b629DFA426114CED6d` |
| Base | 8453 | `0x4C073B3baB6d8826b8C5b229f3cfdC1eC6E47E74` |
| Polygon | 137 | `0x3E14dC1b13c488a8d5D310918780c983bD5982E7` |
| Optimism | 10 | `0x567c4B141ED61923967cA25Ef4906C8781069a10` |
| Arbitrum | 42161 | `0xCf8Acb4eF033efF16E8080aed4c7D5B9285D2192` |
| Avalanche | 43114 | `0x60377C7016E4cdB03C87EF474896C11cB560752C` |
| Celo | 42220 | `0xA4Ff07cF81C02CFD356184879D953970cA957585` |
| BSC | 56 | `0xd1e2cFb6441680002Eb7A44223160aB9B67d7E6E` |
| Base Sepolia | 84532 | `0x109412E3C84f0539b43d39dB691B08c90f58dC7c` |
| Optimism Sepolia | 11155420 | `0xd399e2Fb5f4cf3722a11F65b88FAB6B2B8621005` |
| Avalanche Fuji | 43113 | `0x85Fe79b998509B77BF10A8BD4001D58475D29386` |
| Ethereum Sepolia | 11155111 | `0x109412E3C84f0539b43d39dB691B08c90f58dC7c` |

### SuperTokenFactory (per network)

| Network | Chain ID | Factory Address |
|---------|----------|-----------------|
| Ethereum | 1 | `0x0422689cc4087b6B7280e0a7e7F655200ec86Ae1` |
| Base | 8453 | `0xe20B9a38E0c96F61d1bA6b42a61512D56Fea1Eb3` |
| Polygon | 137 | `0x2C90719f25B10Fc5646c82DA3240C76Fa5BcCF34` |
| Optimism | 10 | `0x8276469A443D5C6B7146BED45e2abCaD3B6adad9` |
| Arbitrum | 42161 | `0x1C21Ead77fd45C84a4c916Db7A6635D0C6FF09D6` |
| Avalanche | 43114 | `0x464AADdBB2B80f3Cb666522EB7381bE610F638b4` |
| Celo | 42220 | `0x36be86dEe6BC726Ed0Cbd170ccD2F21760BC73D9` |
| BSC | 56 | `0x8bde47397301F0Cd31b9000032fD517a39c946Eb` |

---

## Super Token Registry

### Known Super Tokens

#### USDCx Addresses (VERIFIED from Superfluid tokenlist)

| Network | Chain ID | USDCx Address | Underlying USDC |
|---------|----------|---------------|-----------------|
| **Ethereum** | 1 | `0x1BA8603DA702602A8657980e825A6DAa03Dee93a` | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` |
| **Base** | 8453 | `0xD04383398dD2426297da660F9CCA3d439AF9ce1b` | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |
| **Polygon** | 137 | `0xCAa7349CEA390F89641fe306D93591f87595dc1F` | `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` |
| **Optimism** | 10 | `0x8430F084B939208E2eDed1584889C9A66B90562f` | `0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85` |
| **Arbitrum** | 42161 | `0xFC55F2854e74b4f42d01A6D3DAaC4c52D9dFDcFf` | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` |
| **Avalanche** | 43114 | `0x288398f314d472b82C44855F3f6ff20b633C2A97` | `0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E` |
| **BSC** | 56 | `0x0419e1fa3671754F77eC7D5416219a5f9A08b530` | `0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d` |
| **Celo** | 42220 | `0x62b8b11039fcfe5ab0c56e502b1c372a3d2a9c7a` | `0xcebA9300f2b948710d2653dD7B07f33A8B32118C` |

All Super Tokens have 18 decimals. Underlying USDC has 6 decimals (requires x10^12 conversion).

#### UVDx (Avalanche C-Chain) - ULTRAVIOLETA DAO
```
Super Token: 0x11C6AD55Aad69f4612e374e5237b71D580F38f06
Underlying:  0x4Ffe7e01832243e03668E090706F17726c26d6B2
Decimals:    Super=18, Underlying=18 (VERIFIED via Routescan API)
Total Supply: 10,000,000,000 UVD (10 billion)
Network:     Avalanche (43114)
Stats:       284 total streams, ~4.2B total streamed
Note:        NO decimal conversion needed (both 18 decimals)
```

---

## Updated API Schema

### SuperfluidExtra (Extended with Token Selection)

```json
{
  "paymentRequirements": {
    "extra": {
      "superfluid": {
        "super_token": "0xd04383398dd2426297da660f9cca3d439af9ce1b",
        "wrap_amount": "10000000",
        "stream": {
          "recipient": "0x...",
          "flow_rate": "38580246913580",
          "user_data": "subscription-tier-pro"
        },
        "max_fee": "100000"
      }
    }
  }
}
```

### Schema Definition

```rust
/// Superfluid stream configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperfluidStream {
    /// Recipient address for the stream
    pub recipient: EvmAddress,

    /// Flow rate in tokens per second (int96)
    /// Example: 38580246913580 = ~$100/month for 18-decimal token
    pub flow_rate: String,

    /// Optional user data for stream metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
}

/// Superfluid-specific settlement parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperfluidExtra {
    /// Super Token address to wrap to (required)
    /// Example: USDCx on Base = 0xd04383398dd2426297da660f9cca3d439af9ce1b
    pub super_token: EvmAddress,

    /// Amount to wrap (in underlying token decimals)
    pub wrap_amount: TokenAmount,

    /// Stream configuration (optional - if omitted, only wrap is performed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<SuperfluidStream>,

    /// Maximum acceptable fee (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee: Option<TokenAmount>,
}
```

---

## Architecture Overview

### Component Diagram

```
                    ┌─────────────────────────────────────────────┐
                    │              x402-rs Facilitator            │
                    │                                             │
    POST /settle    │  ┌─────────────┐    ┌──────────────────┐   │
    ──────────────>│  │  handlers   │───>│ FacilitatorLocal │   │
                    │  └─────────────┘    └────────┬─────────┘   │
                    │                              │              │
                    │        ┌─────────────────────┼──────────────┤
                    │        │                     │              │
                    │        v                     v              │
                    │  ┌──────────┐         ┌──────────────┐     │
                    │  │ Evm      │         │ Superfluid   │     │
                    │  │ Provider │         │ Provider     │     │
                    │  └────┬─────┘         └──────┬───────┘     │
                    │       │                      │              │
                    └───────┼──────────────────────┼──────────────┘
                            │                      │
                            v                      v
                    ┌───────────────┐      ┌──────────────────┐
                    │  EIP-3009     │      │  Superfluid      │
                    │  USDC/UVD     │      │  Host + CFA      │
                    │  Transfer     │      │  Forwarder       │
                    └───────────────┘      └──────────────────┘
```

### Request Flow

1. **Detect Superfluid Request**: Check `extra.superfluid` in PaymentRequirements
2. **Validate Super Token**: Lookup known Super Token or query on-chain
3. **Calculate Fee**: `max(0.1 underlying, 0.1% of wrap_amount)`
4. **Receive Underlying**: EIP-3009 `transferWithAuthorization` (existing logic)
5. **Approve & Wrap**: `underlying.approve()` + `superToken.upgrade()`
6. **Transfer to User**: `superToken.transfer()` (wrap_amount - fee kept by facilitator)
7. **Create Stream** (optional): `cfaForwarder.createFlow()`
8. **Return Response**: Transaction hashes, amounts, stream status

---

## Implementation Phases Overview

| Phase | What It Includes | Smart Contracts? | Effort |
|-------|------------------|------------------|--------|
| **Phase 1** | ALL Rust code: wrap, transfer, stream, ACL, security fixes, staged deployment | **NO** | 9-10 days |
| **Phase 2** | SuperfluidEscrow.sol + escrow endpoints + security audit | **YES** | +12-15 days |

### Feature Split

| Feature | Phase | Requires Contract? |
|---------|-------|-------------------|
| Superfluid contract addresses registry | 1 | No |
| Known Super Tokens registry (USDCx, UVDx) | 1 | No |
| Wrap underlying → Super Token | 1 | No |
| Transfer Super Tokens to user | 1 | No |
| Create stream (ACL-based) | 1 | No |
| ACL permission checking | 1 | No |
| Pre-flight validation | 1 | No |
| Error recovery + graceful degradation | 1 | No |
| Detailed response with all tx hashes | 1 | No |
| Multi-network deployment (12 networks) | 1 | No |
| Landing page API documentation | 1 | No |
| **SuperfluidEscrow.sol contract** | **2** | **YES** |
| **Trustless refunds** | **2** | **YES** |
| **Pro-rata refund calculation** | **2** | **YES** |
| **Escrow-backed deposits** | **2** | **YES** |
| **Facilitator-managed streams** | **2** | **YES** |

**Phase 1**: Complete Superfluid integration (everything x402-sf does + better)
**Phase 2**: ONLY the escrow contract and its Rust integration

---

## PHASE 1: Gasless Super Token Wrapping Service (No Smart Contracts)

**What this delivers**: Users can gaslessly convert USDC/UVD to USDCx/UVDx Super Tokens.
**What this does NOT deliver**: Automatic streaming (user creates streams manually).

**Deployment**: Staged rollout (NOT all networks at once)

### Networks Enabled (All at Once)

| Mainnets | Testnets |
|----------|----------|
| Ethereum | Ethereum Sepolia |
| Base | Base Sepolia |
| Polygon | Optimism Sepolia |
| Optimism | Avalanche Fuji |
| Arbitrum | |
| Avalanche | |
| Celo | |
| BSC | |

### Phase 1 Implementation Steps

#### Step 1: Superfluid Contracts Module (Day 1)

**File**: `src/chain/superfluid_contracts.rs`

```rust
use alloy::primitives::{address, Address};
use crate::network::Network;

/// Superfluid contract addresses for a network
#[derive(Debug, Clone)]
pub struct SuperfluidContracts {
    pub host: Address,
    pub cfa_forwarder: Address,
    pub super_token_factory: Address,
}

impl SuperfluidContracts {
    /// Get Superfluid contracts for a network
    /// Returns None if Superfluid is not deployed on this network
    pub fn for_network(network: Network) -> Option<Self> {
        match network {
            Network::Ethereum => Some(Self {
                host: address!("4E583d9390082B65Bef884b629DFA426114CED6d"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("0422689cc4087b6B7280e0a7e7F655200ec86Ae1"),
            }),
            Network::Base => Some(Self {
                host: address!("4C073B3baB6d8826b8C5b229f3cfdC1eC6E47E74"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("e20B9a38E0c96F61d1bA6b42a61512D56Fea1Eb3"),
            }),
            Network::Polygon => Some(Self {
                host: address!("3E14dC1b13c488a8d5D310918780c983bD5982E7"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("2C90719f25B10Fc5646c82DA3240C76Fa5BcCF34"),
            }),
            Network::Optimism => Some(Self {
                host: address!("567c4B141ED61923967cA25Ef4906C8781069a10"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("8276469A443D5C6B7146BED45e2abCaD3B6adad9"),
            }),
            Network::Arbitrum => Some(Self {
                host: address!("Cf8Acb4eF033efF16E8080aed4c7D5B9285D2192"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("1C21Ead77fd45C84a4c916Db7A6635D0C6FF09D6"),
            }),
            Network::Avalanche => Some(Self {
                host: address!("60377C7016E4cdB03C87EF474896C11cB560752C"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("464AADdBB2B80f3Cb666522EB7381bE610F638b4"),
            }),
            Network::Celo => Some(Self {
                host: address!("A4Ff07cF81C02CFD356184879D953970cA957585"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("36be86dEe6BC726Ed0Cbd170ccD2F21760BC73D9"),
            }),
            Network::Bsc => Some(Self {
                host: address!("d1e2cFb6441680002Eb7A44223160aB9B67d7E6E"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("8bde47397301F0Cd31b9000032fD517a39c946Eb"),
            }),
            // Testnets
            Network::BaseSepolia => Some(Self {
                host: address!("109412E3C84f0539b43d39dB691B08c90f58dC7c"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("7447E94Dfe3d804a9f46Bf12838d467c912C8F6C"),
            }),
            Network::OptimismSepolia => Some(Self {
                host: address!("d399e2Fb5f4cf3722a11F65b88FAB6B2B8621005"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("fcF0489488397332579f35b0F711BE570Da0E8f5"),
            }),
            Network::AvalancheFuji => Some(Self {
                host: address!("85Fe79b998509B77BF10A8BD4001D58475D29386"),
                // NOTE: Fuji uses different CFA Forwarder!
                cfa_forwarder: address!("2CDd45c5182602a36d391F7F16DD9f8386C3bD8D"),
                super_token_factory: address!("1C92042426B6bAAe497bEf461B6d8342D03aEc92"),
            }),
            Network::EthereumSepolia => Some(Self {
                host: address!("109412E3C84f0539b43d39dB691B08c90f58dC7c"),
                cfa_forwarder: address!("cfA132E353cB4E398080B9700609bb008eceB125"),
                super_token_factory: address!("254C2e152E8602839D288A7bccdf3d0974597193"),
            }),
            // Networks without Superfluid
            _ => None,
        }
    }

    /// Check if a network supports Superfluid
    pub fn is_supported(network: Network) -> bool {
        Self::for_network(network).is_some()
    }
}
```

#### Step 2: Known Super Tokens Registry (Day 1-2)

**File**: `src/chain/super_tokens.rs`

```rust
use alloy::primitives::{address, Address};
use crate::network::Network;

/// A registered Super Token with known addresses
#[derive(Debug, Clone)]
pub struct KnownSuperToken {
    pub symbol: &'static str,
    pub name: &'static str,
    pub super_token: Address,
    pub underlying: Address,
    pub underlying_decimals: u8,
    pub super_decimals: u8,  // Always 18 for Superfluid
}

/// Registry of known Super Tokens per network
pub fn known_super_tokens(network: Network) -> Vec<KnownSuperToken> {
    match network {
        Network::Ethereum => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("1BA8603DA702602A8657980e825A6DAa03Dee93a"),
                underlying: address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        Network::Base => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("D04383398dD2426297da660F9CCA3d439AF9ce1b"),
                underlying: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        Network::Polygon => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("CAa7349CEA390F89641fe306D93591f87595dc1F"),
                underlying: address!("3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        Network::Optimism => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("8430F084B939208E2eDed1584889C9A66B90562f"),
                underlying: address!("0b2C639c533813f4Aa9D7837CAf62653d097Ff85"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        Network::Arbitrum => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("FC55F2854e74b4f42d01A6D3DAaC4c52D9dFDcFf"),
                underlying: address!("af88d065e77c8cC2239327C5EDb3A432268e5831"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        Network::Avalanche => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("288398f314d472b82C44855F3f6ff20b633C2A97"),
                underlying: address!("B97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
            KnownSuperToken {
                symbol: "UVDx",
                name: "Super UltravioletaDAO",
                super_token: address!("11C6AD55Aad69f4612e374e5237b71D580F38f06"),
                underlying: address!("4Ffe7e01832243e03668E090706F17726c26d6B2"),
                underlying_decimals: 18,  // VERIFIED: 10B supply, 18 decimals
                super_decimals: 18,       // No conversion needed!
            },
        ],
        Network::Bsc => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("0419e1fa3671754F77eC7D5416219a5f9A08b530"),
                underlying: address!("8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        Network::Celo => vec![
            KnownSuperToken {
                symbol: "USDCx",
                name: "Super USD Coin",
                super_token: address!("62b8b11039fcfe5ab0c56e502b1c372a3d2a9c7a"),
                underlying: address!("cebA9300f2b948710d2653dD7B07f33A8B32118C"),
                underlying_decimals: 6,
                super_decimals: 18,
            },
        ],
        // Networks without Superfluid or without USDCx
        _ => vec![],
    }
}

/// Lookup a Super Token by its address
pub fn lookup_super_token(network: Network, super_token: Address) -> Option<KnownSuperToken> {
    known_super_tokens(network)
        .into_iter()
        .find(|t| t.super_token == super_token)
}
```

#### Step 3: Superfluid Provider (Day 2-3)

**File**: `src/chain/superfluid.rs`

Core implementation based on the x402-sf reference:

```rust
use alloy::sol;

// ABI for ISuperToken (wrap/upgrade operations)
sol! {
    #[derive(Debug)]
    #[sol(rpc)]
    interface ISuperToken {
        function upgrade(uint256 amount) external;
        function upgradeTo(address to, uint256 amount, bytes calldata userData) external;
        function downgrade(uint256 amount) external;
        function getUnderlyingToken() external view returns (address);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

// ABI for CFAv1Forwarder (stream operations)
sol! {
    #[derive(Debug)]
    #[sol(rpc)]
    interface ICFAv1Forwarder {
        function createFlow(
            address token,
            address sender,
            address receiver,
            int96 flowRate,
            bytes calldata userData
        ) external returns (bool);

        function updateFlow(
            address token,
            address sender,
            address receiver,
            int96 flowRate,
            bytes calldata userData
        ) external returns (bool);

        function deleteFlow(
            address token,
            address sender,
            address receiver,
            bytes calldata userData
        ) external returns (bool);

        function getFlowrate(
            address token,
            address sender,
            address receiver
        ) external view returns (int96);

        function setFlowrateFrom(
            address token,
            address sender,
            address receiver,
            int96 flowrate,
            bytes calldata userData
        ) external returns (bool);
    }
}

// ABI for ICFA (permission checking)
sol! {
    #[derive(Debug)]
    #[sol(rpc)]
    interface ICFA {
        function getFlowOperatorData(
            address token,
            address sender,
            address flowOperator
        ) external view returns (bytes32, uint8, int96);
    }
}

/// Superfluid provider for wrap + stream operations
pub struct SuperfluidProvider<P> {
    inner: EvmProvider<P>,
    contracts: SuperfluidContracts,
    network: Network,
}

impl<P: Provider> SuperfluidProvider<P> {
    /// Settle with Superfluid (wrap + optional stream)
    pub async fn settle_superfluid(
        &self,
        request: &SettleRequest,
        sf_extra: &SuperfluidExtra,
    ) -> Result<SettleResponse, FacilitatorLocalError> {
        // 1. Lookup or query Super Token info
        let super_token_info = self.get_super_token_info(sf_extra.super_token).await?;

        // 2. Calculate fee
        let fee = calculate_fee(sf_extra.wrap_amount, super_token_info.underlying_decimals);
        let total_required = sf_extra.wrap_amount + fee;

        // 3. Validate max_fee if provided
        if let Some(max_fee) = sf_extra.max_fee {
            if fee > max_fee {
                return Err(FacilitatorLocalError::InvalidPayload(
                    format!("Fee {} exceeds max_fee {}", fee, max_fee)
                ));
            }
        }

        // 4. Receive underlying via EIP-3009
        let mut tx_hashes = vec![];
        let receive_tx = self.inner.execute_transfer_with_authorization(request).await?;
        tx_hashes.push(receive_tx);

        // 5. Wrap: approve + upgrade
        let wrap_amount_super = convert_decimals(
            sf_extra.wrap_amount,
            super_token_info.underlying_decimals,
            super_token_info.super_decimals,
        );

        // Approve underlying to Super Token
        let approve_tx = self.approve_underlying(
            super_token_info.underlying,
            sf_extra.super_token,
            sf_extra.wrap_amount,
        ).await?;
        tx_hashes.push(approve_tx);

        // Upgrade (wrap) underlying to Super Token
        let wrap_tx = self.wrap_to_super_token(
            sf_extra.super_token,
            wrap_amount_super,
            &request.payment_payload.payer(),
        ).await?;
        tx_hashes.push(wrap_tx);

        // 6. Create stream (if requested)
        if let Some(stream) = &sf_extra.stream {
            let flow_rate: i96 = stream.flow_rate.parse()
                .map_err(|_| FacilitatorLocalError::InvalidPayload("Invalid flow_rate".into()))?;

            // Check if facilitator has ACL permissions
            let has_permission = self.check_flow_permissions(
                sf_extra.super_token,
                &request.payment_payload.payer(),
            ).await?;

            if has_permission {
                let stream_tx = self.create_flow(
                    sf_extra.super_token,
                    &request.payment_payload.payer(),
                    &stream.recipient,
                    flow_rate,
                    stream.user_data.as_deref(),
                ).await?;
                tx_hashes.push(stream_tx);

                tracing::info!(
                    sender = %request.payment_payload.payer(),
                    recipient = %stream.recipient,
                    flow_rate = %flow_rate,
                    "Superfluid stream created"
                );
            } else {
                tracing::warn!(
                    sender = %request.payment_payload.payer(),
                    "Skipping stream creation: facilitator lacks ACL permissions"
                );
            }
        }

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer: request.payment_payload.payer(),
            transaction: tx_hashes.last().cloned(),
            network: request.payment_payload.network,
        })
    }

    /// Convert decimals (e.g., USDC 6 -> USDCx 18)
    fn convert_decimals(amount: TokenAmount, from_decimals: u8, to_decimals: u8) -> TokenAmount {
        if to_decimals > from_decimals {
            let multiplier = 10u128.pow((to_decimals - from_decimals) as u32);
            amount * multiplier
        } else if from_decimals > to_decimals {
            let divisor = 10u128.pow((from_decimals - to_decimals) as u32);
            amount / divisor
        } else {
            amount
        }
    }
}

/// Calculate fee: max(0.1 underlying, 0.1% of amount)
fn calculate_fee(amount: TokenAmount, decimals: u8) -> TokenAmount {
    let min_fee = TokenAmount::from(10u128.pow(decimals as u32) / 10); // 0.1 token
    let percent_fee = amount / 1000; // 0.1%
    std::cmp::max(min_fee, percent_fee)
}

/// Calculate flow rate from monthly amount
/// monthly_amount: in Super Token decimals (18)
/// Returns: flow rate in tokens/second (int96)
pub fn monthly_to_flow_rate(monthly_amount: TokenAmount) -> i96 {
    const SECONDS_PER_MONTH: u64 = 30 * 24 * 60 * 60; // 2592000
    let rate = u128::from(monthly_amount) / SECONDS_PER_MONTH as u128;
    rate as i96
}
```

#### Step 4: Integration & Testing (Day 4-5)

**Files to modify**:
- `src/chain/mod.rs` - Add superfluid module
- `src/types.rs` - Add SuperfluidExtra, SuperfluidStream types
- `src/facilitator_local.rs` - Route Superfluid requests
- `src/handlers.rs` - Extend /supported endpoint
- `static/index.html` - Add Superfluid API documentation section

**Test files**:
- `tests/integration/test_superfluid_base.py`
- `tests/integration/test_superfluid_avalanche.py`
- `tests/integration/test_uvdx_streaming.py`

#### Step 4b: Landing Page Documentation

Update `static/index.html` to document the new Superfluid endpoints (similar to Escrow section):

**New section to add** (after Escrow API section):

```html
<!-- Superfluid Streaming API -->
<div class="api-section">
  <h3>Superfluid Streaming (x402sf Extension)</h3>
  <p>Enable streaming payments and subscriptions via Superfluid protocol.</p>

  <div class="endpoint">
    <span class="method post">POST</span>
    <span class="path">/settle</span>
    <span class="description">with <code>extra.superfluid</code></span>
  </div>

  <h4>Wrap USDC → USDCx</h4>
  <pre><code>{
  "paymentRequirements": {
    "scheme": "exact",
    "network": "base-mainnet",
    "asset": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
    "extra": {
      "superfluid": {
        "super_token": "0xD04383398dD2426297da660F9CCA3d439AF9Ce1b",
        "wrap_amount": "10000000"
      }
    }
  }
}</code></pre>

  <h4>Wrap + Start Stream</h4>
  <pre><code>{
  "paymentRequirements": {
    "extra": {
      "superfluid": {
        "super_token": "0xD04383398dD2426297da660F9CCA3d439AF9Ce1b",
        "wrap_amount": "10000000",
        "stream": {
          "recipient": "0x...",
          "flow_rate": "38580246913580"
        }
      }
    }
  }
}</code></pre>

  <h4>Supported Super Tokens</h4>
  <table>
    <tr><th>Network</th><th>Token</th><th>Super Token</th></tr>
    <tr><td>Base</td><td>USDC</td><td>USDCx</td></tr>
    <tr><td>Polygon</td><td>USDC</td><td>USDCx</td></tr>
    <tr><td>Optimism</td><td>USDC</td><td>USDCx</td></tr>
    <tr><td>Arbitrum</td><td>USDC</td><td>USDCx</td></tr>
    <tr><td>Avalanche</td><td>USDC</td><td>USDCx</td></tr>
    <tr><td>Avalanche</td><td>UVD</td><td>UVDx</td></tr>
    <!-- ... more rows ... -->
  </table>
</div>
```

**Styling**: Reuse existing `.api-section`, `.endpoint`, `.method`, `.post` CSS classes.

#### Step 5: Security Requirements (Day 5-6)

**Must fix before deployment** (from security-auditor review):

| ID | Issue | Fix |
|----|-------|-----|
| P1-1 | ACL check ignores flowRateAllowance | Check `allowance >= requested_rate` |
| P1-2 | Negative flow rate accepted | Validate `flow_rate > 0` |
| P1-3 | No Super Token verification | Query `SuperTokenFactory.isValidSuperToken()` |
| P1-4 | Decimal overflow risk | Use `checked_mul()` |
| P1-5 | Ambiguous success response | Add `partial_success` field |

#### Step 6: Staged Deployment (Day 7-10)

**DO NOT deploy to all networks at once.** Use staged rollout:

```bash
# Week 1: Testnet only
SUPERFLUID_NETWORKS=base-sepolia
./scripts/build-and-push.sh v1.20.0-beta.1

# Week 2: Add remaining testnets
SUPERFLUID_NETWORKS=base-sepolia,optimism-sepolia,avalanche-fuji,ethereum-sepolia

# Week 3: First mainnet (monitor 72 hours)
SUPERFLUID_NETWORKS=base-sepolia,optimism-sepolia,avalanche-fuji,base

# Week 4+: Staged mainnet rollout
SUPERFLUID_NETWORKS=...,polygon,optimism,arbitrum,avalanche,ethereum,celo,bsc
```

**Feature flag in .env:**
```bash
ENABLE_SUPERFLUID=true
SUPERFLUID_NETWORKS=base-sepolia  # Start small, expand gradually
```

**What Phase 1 enables:**
- Gasless wrap USDC/UVD → USDCx/UVDx
- Transfer Super Tokens to users
- Multi-token support (USDCx on all networks, UVDx on Avalanche)
- User creates streams manually via Superfluid Dashboard

### Phase 1 Summary

**All Phase 1 Steps (Rust only, no smart contracts):**

| Step | Description | Day |
|------|-------------|-----|
| 1 | Superfluid Contracts Module | 1 |
| 2 | Known Super Tokens Registry | 1-2 |
| 3 | Superfluid Provider (extend EvmProvider, NOT new provider) | 2-4 |
| 4 | Integration & Testing | 4-5 |
| 4b | Landing Page Documentation | 5 |
| 5 | Security Requirements (P1-1 through P1-5) | 5-6 |
| 6 | Staged Deployment (testnets first, then mainnets) | 7-10 |
| 7 | Error Recovery Strategy | (included in 3-4) |
| 8 | ACL Flow Documentation | (included in 3-4) |

**What Phase 1 Delivers:**

| Feature | Included |
|---------|----------|
| Gasless wrap + transfer | YES |
| Stream creation (ACL-based) | YES |
| Multi-network (8 mainnets + 4 testnets) | YES |
| Multi-token (USDCx, UVDx) | YES |
| Pre-flight ACL validation | YES |
| Error recovery instructions | YES |
| Detailed tx hash responses | YES |
| Landing page API documentation | YES |

**What Requires Phase 2 (Smart Contract):**

| Feature | Requires Contract |
|---------|-------------------|
| Trustless refunds | YES |
| Pro-rata refund calculation | YES |
| Facilitator-managed streams | YES |
| Escrow-backed deposits | YES |

**Total Phase 1 Effort**: 5-7 days of Rust development, single deployment to all networks

---

## Example Usage

### Wrap USDC to USDCx (Base)

```json
{
  "x402Version": 1,
  "paymentPayload": { /* EIP-3009 signature for 10.1 USDC */ },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "base",
    "asset": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
    "maxAmountRequired": "10100000",
    "extra": {
      "superfluid": {
        "super_token": "0xd04383398dd2426297da660f9cca3d439af9ce1b",
        "wrap_amount": "10000000"
      }
    }
  }
}
```

### Stream UVDx to Member (Avalanche)

```json
{
  "x402Version": 1,
  "paymentPayload": { /* EIP-3009 signature for UVD */ },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "avalanche",
    "asset": "0x4Ffe7e01832243e03668E090706F17726c26d6B2",
    "maxAmountRequired": "1000000000000000000000",
    "extra": {
      "superfluid": {
        "super_token": "0x11C6AD55Aad69f4612e374e5237b71D580F38f06",
        "wrap_amount": "1000000000000000000000",
        "stream": {
          "recipient": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb",
          "flow_rate": "38580246913580"
        }
      }
    }
  }
}
```

---

## Open Questions

1. ~~**UVD Token Decimals**: Verify if UVD is 18 decimals~~ **RESOLVED: UVD = 18 decimals (verified via Routescan API)**
2. ~~**USDCx Addresses**: Need to verify USDCx addresses on all networks~~ **RESOLVED: All verified from Superfluid tokenlist**
3. ~~**ACL Flow**: How to prompt users to grant facilitator ACL permissions?~~ **RESOLVED: See ACL Flow section below**
4. ~~**Error Recovery**: If wrap succeeds but stream fails, how to handle?~~ **RESOLVED: See Error Recovery Strategy below**

---

### Step 6: Error Recovery Strategy (Phase 1)

#### Problem Analysis

The Superfluid wrap+stream flow is **NOT atomic** - it consists of multiple on-chain transactions:

1. **EIP-3009 Transfer** - Move underlying tokens (USDC/UVD) to facilitator
2. **Approve** - Approve Super Token contract to spend underlying
3. **Wrap (upgrade)** - Convert underlying → Super Token
4. **Transfer** - Send Super Tokens to user
5. **Create Stream** (optional) - Set up flow from user to recipient

If step 5 fails after steps 1-4 succeed, user has Super Tokens but no stream.

#### Reference Implementation Analysis

**x402-sf (Superfluid's official implementation):**
- Validates ACL permissions BEFORE attempting stream
- No documented recovery if stream creation fails post-wrap
- Users end up with Super Tokens in wallet (not ideal but not fund loss)

**Our x402r Escrow (existing):**
- Deposits go through deterministic proxy contracts
- Escrow holds funds until release or refund
- Dispute windows allow recovery

#### Recommended Strategy: Defensive Validation + Graceful Degradation

**Principle**: Validate everything possible BEFORE any on-chain action, but design for graceful fallback.

#### Pre-Flight Validation (CRITICAL)

Before ANY transaction, validate:

```rust
pub async fn validate_superfluid_request(
    &self,
    sf_extra: &SuperfluidExtra,
    payer: &Address,
) -> Result<SuperfluidValidation, SuperfluidError> {
    // 1. Verify Super Token is legitimate
    let is_valid_super_token = self.verify_super_token(sf_extra.super_token).await?;
    if !is_valid_super_token {
        return Err(SuperfluidError::InvalidSuperToken);
    }

    // 2. Check ACL permissions BEFORE wrapping (if stream requested)
    if sf_extra.stream.is_some() {
        let has_permission = self.check_flow_permissions(
            sf_extra.super_token,
            payer,
        ).await?;

        if !has_permission {
            return Err(SuperfluidError::MissingAclPermission {
                super_token: sf_extra.super_token,
                user: *payer,
                message: "User must grant ACL permissions before streaming. \
                         Call grantPermissions() on CFAv1Forwarder first.".into(),
            });
        }
    }

    // 3. Check recipient is not blacklisted (if stream requested)
    if let Some(stream) = &sf_extra.stream {
        if self.is_blacklisted(&stream.recipient).await {
            return Err(SuperfluidError::RecipientBlacklisted);
        }
    }

    // 4. Validate flow rate is reasonable
    if let Some(stream) = &sf_extra.stream {
        let flow_rate: i96 = stream.flow_rate.parse()
            .map_err(|_| SuperfluidError::InvalidFlowRate)?;

        if flow_rate <= 0 {
            return Err(SuperfluidError::InvalidFlowRate);
        }

        // Check flow rate doesn't exceed wrap amount per reasonable period
        // (e.g., shouldn't drain in less than 1 hour)
        let min_duration_seconds = 3600; // 1 hour
        let min_wrap_for_rate = (flow_rate as u128) * min_duration_seconds;
        if sf_extra.wrap_amount < min_wrap_for_rate.into() {
            return Err(SuperfluidError::InsufficientWrapForFlowRate);
        }
    }

    Ok(SuperfluidValidation::Valid)
}
```

#### Transactional Approach with Clear Outcomes

```rust
pub async fn settle_superfluid(
    &self,
    request: &SettleRequest,
    sf_extra: &SuperfluidExtra,
) -> Result<SuperfluidSettleResponse, SuperfluidError> {
    // Pre-flight validation
    self.validate_superfluid_request(sf_extra, &request.payer()).await?;

    let mut result = SuperfluidSettleResponse::default();

    // Step 1: Receive underlying tokens
    let receive_tx = self.receive_underlying(request).await?;
    result.receive_tx = Some(receive_tx);
    result.underlying_received = true;

    // Step 2: Wrap to Super Token
    let wrap_tx = self.wrap_tokens(sf_extra).await?;
    result.wrap_tx = Some(wrap_tx);
    result.tokens_wrapped = true;

    // Step 3: Transfer Super Tokens to user
    let transfer_tx = self.transfer_super_tokens(
        sf_extra.super_token,
        &request.payer(),
        sf_extra.wrap_amount,
    ).await?;
    result.transfer_tx = Some(transfer_tx);
    result.tokens_transferred = true;

    // Step 4: Create stream (if requested AND validated)
    if let Some(stream) = &sf_extra.stream {
        match self.create_flow_for_user(
            sf_extra.super_token,
            &request.payer(),
            &stream.recipient,
            stream.flow_rate.parse().unwrap(),
            stream.user_data.as_deref(),
        ).await {
            Ok(stream_tx) => {
                result.stream_tx = Some(stream_tx);
                result.stream_created = true;
            }
            Err(e) => {
                // Stream failed but user has their Super Tokens
                // Log warning but don't fail the settlement
                tracing::warn!(
                    payer = %request.payer(),
                    recipient = %stream.recipient,
                    error = %e,
                    "Stream creation failed - user has Super Tokens but no stream"
                );
                result.stream_error = Some(e.to_string());
                result.stream_created = false;
            }
        }
    }

    // Return success - user has their tokens even if stream failed
    Ok(result)
}
```

#### Response Structure for Transparency

```rust
/// Superfluid settlement response with detailed status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperfluidSettleResponse {
    /// Overall success (true if user received Super Tokens)
    pub success: bool,

    /// Transaction receiving underlying tokens
    pub receive_tx: Option<TransactionHash>,
    pub underlying_received: bool,

    /// Transaction wrapping to Super Token
    pub wrap_tx: Option<TransactionHash>,
    pub tokens_wrapped: bool,

    /// Transaction transferring Super Tokens to user
    pub transfer_tx: Option<TransactionHash>,
    pub tokens_transferred: bool,

    /// Stream creation (optional)
    pub stream_tx: Option<TransactionHash>,
    pub stream_created: bool,
    pub stream_error: Option<String>,

    /// If stream failed, instructions for user
    pub recovery_instructions: Option<String>,
}

impl SuperfluidSettleResponse {
    pub fn with_stream_recovery(mut self) -> Self {
        if !self.stream_created && self.tokens_transferred {
            self.recovery_instructions = Some(
                "Stream creation failed but you received your Super Tokens. \
                 You can manually create a stream using the Superfluid Dashboard: \
                 https://app.superfluid.finance/".into()
            );
        }
        self
    }
}
```

#### Why NOT Use Escrow for Basic Superfluid (Phase 1)?

Our x402r escrow is excellent for **refundable one-time payments**, but Superfluid streaming is different:

| x402r Escrow | Superfluid Streams |
|--------------|-------------------|
| One-time payment | Continuous streaming |
| Binary outcome (refund or release) | Ongoing flow |
| Dispute window makes sense | No "dispute" concept in streams |
| Buyer might want money back | User wants tokens to stream |

**Key insight**: If wrap succeeds but stream fails, the user still has their Super Tokens (USDCx/UVDx) in their wallet. They can:
1. Manually create a stream via Superfluid Dashboard
2. Use the tokens for something else
3. Unwrap back to underlying tokens

This is **not fund loss** - just an incomplete operation that the user can finish.

#### Implementation Priority

1. **Mandatory**: Pre-flight ACL validation (prevent most stream failures)
2. **Mandatory**: Detailed response with all transaction hashes
3. **Recommended**: Recovery instructions if stream fails

---

### Step 7: ACL (Access Control List) Flow (Phase 1)

The Superfluid ACL system allows the facilitator to create/update/delete streams on behalf of users. This is necessary because after wrapping tokens to Super Tokens and transferring them to the user's wallet, the facilitator needs permission to create a stream FROM the user's wallet.

#### Permission Model

Superfluid uses a permission bitmask system:
- **Bit 0 (1)**: Permission to CREATE flows
- **Bit 1 (2)**: Permission to UPDATE flows
- **Bit 2 (4)**: Permission to DELETE flows
- **All permissions (7)**: Full control (create + update + delete)

Additionally, operators can have a **flowRateAllowance** - a "tank" of flow rate that depletes as the operator creates/increases flows.

#### CFAv1Forwarder ACL Functions

```solidity
/// Grant FULL permissions to an operator (create + update + delete)
/// User calls this to allow facilitator to manage their streams
function grantPermissions(
    ISuperToken token,       // The Super Token (e.g., USDCx)
    address flowOperator     // The facilitator address
) external returns (bool);

/// Revoke all permissions from an operator
function revokePermissions(
    ISuperToken token,
    address flowOperator
) external returns (bool);

/// Grant GRANULAR permissions with flow rate limit
/// More secure - can limit what operator can do
function updateFlowOperatorPermissions(
    ISuperToken token,
    address flowOperator,
    uint8 permissions,        // Bitmask: 1=create, 2=update, 4=delete
    int96 flowrateAllowance   // Max flow rate operator can allocate
) external returns (bool);

/// Create a flow AS an operator (requires ACL permission)
function createFlow(
    ISuperToken token,
    address sender,           // User's address (the one who granted permission)
    address receiver,         // Stream recipient
    int96 flowrate,           // Tokens per second
    bytes calldata userData   // Optional metadata
) external returns (bool);

/// Set flow rate from sender to receiver (operator version)
function setFlowrateFrom(
    ISuperToken token,
    address sender,
    address receiver,
    int96 flowrate
) external returns (bool);
```

#### User Flow for Stream Creation

**Option A: User Pre-Grants Permissions (Recommended)**

1. User visits a "Setup Streaming" page in the client app
2. User signs a transaction calling `grantPermissions(USDCx, facilitatorAddress)`
3. User can now use x402 payments that include streaming
4. Facilitator wraps tokens and creates stream in a single settlement

**Option B: Wrap-Only, User Creates Stream**

1. Client sends x402 payment with `stream: null`
2. Facilitator wraps tokens and sends Super Tokens to user
3. User manually creates stream via Superfluid Dashboard or their own tx

#### Checking Permissions

```solidity
// From IConstantFlowAgreementV1 (accessed via Host.getAgreement)
function getFlowOperatorData(
    ISuperToken token,
    address sender,           // User address
    address flowOperator      // Facilitator address
) external view returns (
    bytes32 flowOperatorId,
    uint8 permissions,        // Current permission bitmask
    int96 flowRateAllowance   // Remaining allowance
);
```

#### Rust Implementation for Permission Check

```rust
/// Check if facilitator has CREATE permission for user's streams
pub async fn check_flow_permissions(
    &self,
    super_token: Address,
    sender: &Address,
) -> Result<bool, FacilitatorLocalError> {
    // Call getFlowOperatorData on CFA agreement
    let cfa = ICFA::new(self.contracts.cfa_address, &self.provider);

    let (_, permissions, _) = cfa.getFlowOperatorData(
        super_token,
        *sender,
        self.facilitator_address,
    ).call().await?;

    // Check if CREATE permission (bit 0) is set
    let has_create = (permissions & 1) != 0;

    Ok(has_create)
}
```

#### UI Prompt Strategy

For client applications, recommend showing users a one-time setup:

```
To enable automatic streaming payments:

1. Connect your wallet
2. Approve the x402 Facilitator to manage your streams
3. This is a one-time setup per token

[Approve USDCx Streaming] [Approve UVDx Streaming]
```

The approval transaction costs minimal gas and only needs to be done once per Super Token per user.

---

## PHASE 2: x402r + Superfluid Escrow Integration (Requires Smart Contract)

This section documents the complete architecture for combining x402r escrow-backed refundable payments with Superfluid streaming. This requires deploying the `SuperfluidEscrow.sol` contract to all supported networks.

### Why Combine x402r Escrow with Superfluid?

| Feature | Basic Superfluid | x402r Escrow + Superfluid |
|---------|------------------|---------------------------|
| Streaming payments | Yes | Yes |
| Gasless deposits (EIP-3009) | Yes | Yes |
| **Refund capability** | No (user must cancel manually) | **Yes (trustless, on-demand)** |
| **Dispute resolution** | No | **Yes (arbiter system)** |
| **Partial refunds** | No | **Yes (pro-rata based on streamed)** |
| **Subscription management** | User responsibility | **Facilitator managed** |
| **Capital efficiency** | User holds Super Tokens | **Escrow holds, streams out** |

### Architecture Options Analysis

Based on research of Superfluid's VestingSchedulerV2 and Super Apps:

| Approach | How It Works | Pros | Cons |
|----------|--------------|------|------|
| **VestingSchedulerV2 Pattern** | Uses ACL permissions, doesn't hold tokens | Gas efficient, battle-tested | No refund capability |
| **Super App Escrow** | Contract holds Super Tokens, creates streams FROM escrow | Full refund control, trustless | New contract to deploy/audit |
| **Hybrid: Escrow + Permission** | Escrow holds underlying, grants permissions to VestingScheduler | Leverages existing infra | Complex, two contracts |

**Recommendation**: **Super App Escrow** - Custom `SuperfluidEscrow.sol` that holds Super Tokens and manages streams, enabling trustless refunds.

### Architecture Overview

```
                    x402r + SUPERFLUID ESCROW ARCHITECTURE
┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│   USER/BUYER                                                            │
│   ┌─────────────────────────────────────────────────────────────┐       │
│   │  1. Signs EIP-3009 authorization for USDC/UVD transfer      │       │
│   │  2. Specifies: recipient, flow_rate, duration, max_deposit  │       │
│   │  3. Can request refund at any time via /superfluid/refund   │       │
│   └────────────────────────┬────────────────────────────────────┘       │
│                            │                                            │
│                            ▼                                            │
│   ┌─────────────────────────────────────────────────────────────┐       │
│   │                    x402-rs FACILITATOR                      │       │
│   │                                                             │       │
│   │  • Receives EIP-3009 signed payment                         │       │
│   │  • Wraps USDC → USDCx (or UVD → UVDx)                        │       │
│   │  • Deposits USDCx to SuperfluidEscrow contract              │       │
│   │  • Calls escrow.createStreamFromDeposit()                   │       │
│   │  • Handles refund requests via escrow.requestRefund()       │       │
│   └────────────────────────┬────────────────────────────────────┘       │
│                            │                                            │
│                            ▼                                            │
│   ┌─────────────────────────────────────────────────────────────┐       │
│   │              SUPERFLUIDESCROW.SOL (Super App)               │       │
│   │                                                             │       │
│   │  DEPOSITS                         STREAMS                   │       │
│   │  ┌──────────────────────┐        ┌────────────────────┐     │       │
│   │  │ depositId => {       │        │ USDCx streaming    │     │       │
│   │  │   buyer: 0x123...    │───────▶│ FROM Escrow        │     │       │
│   │  │   recipient: 0x456...│        │ TO Recipient       │     │       │
│   │  │   token: USDCx       │        │ @ flowRate/sec     │     │       │
│   │  │   totalDeposit: 100  │        └─────────┬──────────┘     │       │
│   │  │   streamedAmount: 30 │                  │                │       │
│   │  │   flowRate: X/sec    │                  ▼                │       │
│   │  │   status: STREAMING  │        ┌────────────────────┐     │       │
│   │  │ }                    │        │    RECIPIENT       │     │       │
│   │  └──────────────────────┘        │    (Seller)        │     │       │
│   │                                  │  Receives tokens   │     │       │
│   │  REFUND FLOW                     │  continuously      │     │       │
│   │  ┌──────────────────────┐        └────────────────────┘     │       │
│   │  │ On refund request:   │                                   │       │
│   │  │ 1. Stop stream       │                                   │       │
│   │  │ 2. Calculate streamed│                                   │       │
│   │  │ 3. Return remaining  │                                   │       │
│   │  │    to buyer          │                                   │       │
│   │  └──────────────────────┘                                   │       │
│   │                                                             │       │
│   │  SUPER APP CALLBACKS (react to stream events)               │       │
│   │  • onFlowDeleted: mark deposit as completed                 │       │
│   │  • onFlowUpdated: track rate changes                        │       │
│   └─────────────────────────────────────────────────────────────┘       │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### SuperfluidEscrow Contract Design

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { ISuperToken } from "@superfluid-finance/ethereum-contracts/contracts/interfaces/superfluid/ISuperToken.sol";
import { ISuperfluid } from "@superfluid-finance/ethereum-contracts/contracts/interfaces/superfluid/ISuperfluid.sol";
import { CFASuperAppBase } from "@superfluid-finance/ethereum-contracts/contracts/apps/CFASuperAppBase.sol";
import { SuperTokenV1Library } from "@superfluid-finance/ethereum-contracts/contracts/apps/SuperTokenV1Library.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/security/ReentrancyGuard.sol";

/**
 * @title SuperfluidEscrow
 * @notice x402r-compatible escrow that holds Super Tokens and creates streams to recipients
 * @dev Extends CFASuperAppBase to react to stream events via callbacks
 *
 * KEY INSIGHT FROM RESEARCH:
 * - VestingSchedulerV2: Uses permissions, doesn't hold tokens (no refund capability)
 * - SuperfluidEscrow: HOLDS tokens, creates streams FROM escrow balance
 *   This enables trustless refunds - buyer doesn't need to trust recipient
 *
 * SUPER APP CALLBACKS:
 * - onFlowCreated: N/A (we create outgoing flows)
 * - onFlowUpdated: Track rate changes for deposits
 * - onFlowDeleted: Mark deposit as completed when stream ends
 */
contract SuperfluidEscrow is CFASuperAppBase, ReentrancyGuard {
    using SuperTokenV1Library for ISuperToken;

    // ============ Structs ============

    struct Deposit {
        address buyer;              // Who deposited (can request refund)
        address recipient;          // Stream destination (seller)
        ISuperToken superToken;     // USDCx, UVDx, etc.
        uint256 totalDeposit;       // Total deposited amount (18 decimals)
        int96 flowRate;             // Tokens per second
        uint256 startTime;          // When stream started (0 if pending)
        uint256 endTime;            // Estimated end (calculated from deposit/rate)
        DepositStatus status;
        bytes32 x402PaymentId;      // Link to x402 payment for tracking
    }

    enum DepositStatus {
        Pending,            // Deposited, stream not started
        Streaming,          // Active stream from escrow to recipient
        Completed,          // Stream finished (deposit exhausted)
        Refunded,           // Buyer requested refund
        Disputed            // Under dispute resolution (future)
    }

    // ============ State ============

    mapping(bytes32 => Deposit) public deposits;
    mapping(address => bytes32[]) public buyerDeposits;
    mapping(address => bytes32[]) public recipientDeposits;

    // Track active streams per recipient to handle callbacks
    mapping(address => mapping(ISuperToken => bytes32)) public activeStreamDeposit;

    // Protocol fee (basis points, e.g., 50 = 0.5%)
    uint256 public protocolFeeBps = 50;
    address public feeCollector;
    address public facilitator; // Only facilitator can create deposits

    // Minimum deposit to prevent dust attacks
    uint256 public minDeposit = 1e18; // 1 Super Token minimum

    // ============ Events ============

    event DepositCreated(
        bytes32 indexed depositId,
        address indexed buyer,
        address indexed recipient,
        address superToken,
        uint256 amount,
        int96 flowRate,
        bytes32 x402PaymentId
    );

    event StreamStarted(
        bytes32 indexed depositId,
        int96 flowRate,
        uint256 estimatedEndTime
    );

    event RefundProcessed(
        bytes32 indexed depositId,
        uint256 refundedToBuyer,
        uint256 streamedToRecipient,
        uint256 protocolFee
    );

    event DepositCompleted(
        bytes32 indexed depositId,
        uint256 totalStreamed
    );

    // ============ Errors ============

    error DepositNotFound();
    error NotBuyer();
    error NotFacilitator();
    error InvalidStatus();
    error InsufficientDeposit();
    error InvalidFlowRate();
    error StreamAlreadyActive();

    // ============ Constructor ============

    constructor(
        ISuperfluid host,
        address facilitator_,
        address feeCollector_
    ) CFASuperAppBase(host) {
        facilitator = facilitator_;
        feeCollector = feeCollector_;
    }

    // ============ Core Functions ============

    /**
     * @notice Create a deposit and optionally start streaming
     * @dev Only callable by facilitator (after wrapping tokens)
     * @param buyer Address that owns the deposit (can request refund)
     * @param recipient Address to receive the stream
     * @param superToken The Super Token (USDCx, UVDx)
     * @param amount Total amount to deposit (already in Super Token decimals)
     * @param flowRate Tokens per second to stream (int96)
     * @param startStreaming Whether to start stream immediately
     * @param x402PaymentId Link to x402 payment for tracking
     * @return depositId Unique identifier for this deposit
     */
    function createDeposit(
        address buyer,
        address recipient,
        ISuperToken superToken,
        uint256 amount,
        int96 flowRate,
        bool startStreaming,
        bytes32 x402PaymentId
    ) external nonReentrant returns (bytes32 depositId) {
        if (msg.sender != facilitator) revert NotFacilitator();
        if (amount < minDeposit) revert InsufficientDeposit();
        if (flowRate <= 0) revert InvalidFlowRate();

        // Transfer Super Tokens from facilitator to this contract
        superToken.transferFrom(msg.sender, address(this), amount);

        // Calculate estimated end time
        uint256 estimatedEndTime = block.timestamp + (amount / uint256(int256(flowRate)));

        // Generate unique deposit ID
        depositId = keccak256(abi.encodePacked(
            buyer,
            recipient,
            address(superToken),
            block.timestamp,
            x402PaymentId
        ));

        // Store deposit
        deposits[depositId] = Deposit({
            buyer: buyer,
            recipient: recipient,
            superToken: superToken,
            totalDeposit: amount,
            flowRate: flowRate,
            startTime: 0,
            endTime: estimatedEndTime,
            status: DepositStatus.Pending,
            x402PaymentId: x402PaymentId
        });

        // Index for queries
        buyerDeposits[buyer].push(depositId);
        recipientDeposits[recipient].push(depositId);

        emit DepositCreated(
            depositId, buyer, recipient, address(superToken),
            amount, flowRate, x402PaymentId
        );

        // Start streaming if requested
        if (startStreaming) {
            _startStream(depositId);
        }

        return depositId;
    }

    /**
     * @notice Start streaming from a pending deposit
     * @dev Can be called by buyer, recipient, or facilitator
     */
    function startStream(bytes32 depositId) external nonReentrant {
        Deposit storage deposit = deposits[depositId];
        if (deposit.buyer == address(0)) revert DepositNotFound();
        if (deposit.status != DepositStatus.Pending) revert InvalidStatus();

        _startStream(depositId);
    }

    /**
     * @notice Request refund - stops stream and returns remaining balance to buyer
     * @dev Only buyer can request. Calculates pro-rata refund.
     */
    function requestRefund(bytes32 depositId) external nonReentrant {
        Deposit storage deposit = deposits[depositId];
        if (deposit.buyer == address(0)) revert DepositNotFound();
        if (msg.sender != deposit.buyer) revert NotBuyer();
        if (deposit.status != DepositStatus.Streaming &&
            deposit.status != DepositStatus.Pending) {
            revert InvalidStatus();
        }

        // Calculate streamed amount based on elapsed time
        uint256 streamed = 0;
        if (deposit.status == DepositStatus.Streaming) {
            uint256 elapsed = block.timestamp - deposit.startTime;
            streamed = elapsed * uint256(int256(deposit.flowRate));
            if (streamed > deposit.totalDeposit) {
                streamed = deposit.totalDeposit;
            }

            // Stop the stream
            deposit.superToken.deleteFlow(address(this), deposit.recipient);

            // Clear active stream tracking
            activeStreamDeposit[deposit.recipient][deposit.superToken] = bytes32(0);
        }

        // Calculate refund and fee
        uint256 remaining = deposit.totalDeposit - streamed;
        uint256 fee = (remaining * protocolFeeBps) / 10000;
        uint256 refundAmount = remaining - fee;

        // Transfer refund to buyer
        if (refundAmount > 0) {
            deposit.superToken.transfer(deposit.buyer, refundAmount);
        }

        // Transfer fee to collector
        if (fee > 0 && feeCollector != address(0)) {
            deposit.superToken.transfer(feeCollector, fee);
        }

        // Update state
        deposit.status = DepositStatus.Refunded;

        emit RefundProcessed(depositId, refundAmount, streamed, fee);
    }

    /**
     * @notice Get current streamed amount for a deposit
     */
    function getStreamedAmount(bytes32 depositId) external view returns (uint256) {
        Deposit storage deposit = deposits[depositId];
        if (deposit.status == DepositStatus.Pending) {
            return 0;
        }
        if (deposit.status == DepositStatus.Refunded ||
            deposit.status == DepositStatus.Completed) {
            // Already finalized - return 0 (actual amount was recorded)
            return 0;
        }

        // Calculate based on elapsed time
        uint256 elapsed = block.timestamp - deposit.startTime;
        uint256 streamed = elapsed * uint256(int256(deposit.flowRate));
        return streamed > deposit.totalDeposit ? deposit.totalDeposit : streamed;
    }

    /**
     * @notice Get refundable amount (what buyer would receive if they refund now)
     */
    function getRefundableAmount(bytes32 depositId) external view returns (uint256) {
        Deposit storage deposit = deposits[depositId];
        if (deposit.status != DepositStatus.Streaming &&
            deposit.status != DepositStatus.Pending) {
            return 0;
        }

        uint256 streamed = this.getStreamedAmount(depositId);
        uint256 remaining = deposit.totalDeposit - streamed;
        uint256 fee = (remaining * protocolFeeBps) / 10000;
        return remaining - fee;
    }

    // ============ Internal Functions ============

    function _startStream(bytes32 depositId) internal {
        Deposit storage deposit = deposits[depositId];

        // Check no existing stream to this recipient for this token
        bytes32 existingDeposit = activeStreamDeposit[deposit.recipient][deposit.superToken];
        if (existingDeposit != bytes32(0)) {
            revert StreamAlreadyActive();
        }

        // Create stream from escrow to recipient
        deposit.superToken.createFlow(deposit.recipient, deposit.flowRate);

        // Track for callback handling
        activeStreamDeposit[deposit.recipient][deposit.superToken] = depositId;

        deposit.startTime = block.timestamp;
        deposit.status = DepositStatus.Streaming;

        emit StreamStarted(depositId, deposit.flowRate, deposit.endTime);
    }

    // ============ Super App Callbacks ============

    /**
     * @notice Called when an outgoing flow from this contract is deleted
     * @dev This happens when:
     *      1. Stream runs out of funds (liquidation)
     *      2. Refund was processed (we deleted it)
     *      3. External deletion (shouldn't happen)
     */
    function onFlowDeleted(
        ISuperToken superToken,
        address /*sender*/,  // Will be this contract
        address receiver,
        int96 /*previousFlowRate*/,
        uint256 /*lastUpdated*/,
        bytes calldata ctx
    ) internal override returns (bytes memory newCtx) {
        // Find the deposit for this stream
        bytes32 depositId = activeStreamDeposit[receiver][superToken];
        if (depositId != bytes32(0)) {
            Deposit storage deposit = deposits[depositId];

            // Only update if still streaming (not already refunded)
            if (deposit.status == DepositStatus.Streaming) {
                deposit.status = DepositStatus.Completed;
                emit DepositCompleted(depositId, deposit.totalDeposit);
            }

            // Clear tracking
            activeStreamDeposit[receiver][superToken] = bytes32(0);
        }

        return ctx;
    }

    // ============ View Functions ============

    function getDeposit(bytes32 depositId) external view returns (Deposit memory) {
        return deposits[depositId];
    }

    function getBuyerDeposits(address buyer) external view returns (bytes32[] memory) {
        return buyerDeposits[buyer];
    }

    function getRecipientDeposits(address recipient) external view returns (bytes32[] memory) {
        return recipientDeposits[recipient];
    }

    // ============ Admin Functions ============

    function setFacilitator(address newFacilitator) external {
        require(msg.sender == facilitator, "Only facilitator");
        facilitator = newFacilitator;
    }

    function setFeeCollector(address newFeeCollector) external {
        require(msg.sender == facilitator, "Only facilitator");
        feeCollector = newFeeCollector;
    }

    function setProtocolFee(uint256 newFeeBps) external {
        require(msg.sender == facilitator, "Only facilitator");
        require(newFeeBps <= 500, "Fee too high"); // Max 5%
        protocolFeeBps = newFeeBps;
    }
}
```

### Integration with x402-rs Facilitator

#### New Rust Types for Escrow Mode

```rust
/// x402r + Superfluid escrow extension - enables trustless refunds
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperfluidEscrowExtra {
    /// Super Token to use (USDCx, UVDx)
    pub super_token: Address,

    /// Amount to wrap and deposit to escrow
    pub deposit_amount: TokenAmount,

    /// Stream recipient (seller/service provider)
    pub recipient: Address,

    /// Flow rate (tokens per second as string, e.g., "380517503805")
    pub flow_rate: String,

    /// Whether to start streaming immediately (default: true)
    #[serde(default = "default_true")]
    pub start_streaming: bool,

    /// Link to subscription/service metadata (optional)
    pub subscription_id: Option<String>,
}

fn default_true() -> bool { true }

/// Response for escrow-backed streaming settlement
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SuperfluidEscrowResponse {
    pub success: bool,

    /// EIP-3009 transfer tx (underlying received)
    pub receive_tx: Option<String>,

    /// Wrap underlying → Super Token tx
    pub wrap_tx: Option<String>,

    /// Approve escrow to spend Super Tokens tx
    pub approve_tx: Option<String>,

    /// Deposit to SuperfluidEscrow tx
    pub deposit_tx: Option<String>,

    /// Stream creation tx (if start_streaming = true)
    pub stream_tx: Option<String>,

    /// Escrow deposit ID for future operations (refund, query)
    pub deposit_id: Option<String>,

    /// Stream details
    pub stream_info: Option<StreamInfo>,

    /// Error if any step failed
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub recipient: String,
    pub flow_rate: String,
    pub tokens_per_month: String,
    pub estimated_end_time: u64,
    pub refundable_amount: String,
}

/// Request to refund an escrowed stream
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundEscrowRequest {
    pub network: String,
    pub deposit_id: String,
    /// EIP-712 signature proving caller is the buyer
    pub signature: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RefundResponse {
    pub success: bool,
    pub transaction: Option<String>,
    pub refunded_amount: Option<String>,
    pub streamed_amount: Option<String>,
    pub error: Option<String>,
}
```

#### Settlement Flow Implementation

```rust
impl<P: Provider> SuperfluidProvider<P> {
    /// Settle with Superfluid Escrow (wrap + deposit + stream with refund capability)
    pub async fn settle_superfluid_escrow(
        &self,
        request: &SettleRequest,
        escrow_extra: &SuperfluidEscrowExtra,
    ) -> Result<SuperfluidEscrowResponse, SuperfluidError> {
        let mut result = SuperfluidEscrowResponse::default();
        let payer = request.payment_payload.payer();

        // 1. Get Super Token info
        let super_token_info = self.get_super_token_info(escrow_extra.super_token).await?;

        // 2. Receive underlying tokens via EIP-3009
        let receive_tx = self.inner.execute_transfer_with_authorization(request).await?;
        result.receive_tx = Some(format!("0x{}", hex::encode(receive_tx)));

        // 3. Wrap to Super Token
        let wrap_amount_super = convert_decimals(
            escrow_extra.deposit_amount,
            super_token_info.underlying_decimals,
            18, // Super Tokens always 18 decimals
        );
        let wrap_tx = self.wrap_to_super_token(
            escrow_extra.super_token,
            wrap_amount_super,
        ).await?;
        result.wrap_tx = Some(format!("0x{}", hex::encode(wrap_tx)));

        // 4. Approve SuperfluidEscrow to spend Super Tokens
        let escrow_address = self.superfluid_escrow_for_network()?;
        let approve_tx = self.approve(
            escrow_extra.super_token,
            escrow_address,
            wrap_amount_super,
        ).await?;
        result.approve_tx = Some(format!("0x{}", hex::encode(approve_tx)));

        // 5. Create deposit in escrow contract
        let flow_rate: i96 = escrow_extra.flow_rate.parse()
            .map_err(|_| SuperfluidError::InvalidFlowRate)?;

        let x402_payment_id = keccak256(
            format!("{}-{}-{}", payer, escrow_extra.recipient, request.nonce()).as_bytes()
        );

        let deposit_result = self.create_escrow_deposit(
            escrow_address,
            payer,
            escrow_extra.recipient,
            escrow_extra.super_token,
            wrap_amount_super,
            flow_rate,
            escrow_extra.start_streaming,
            x402_payment_id,
        ).await?;

        result.deposit_tx = Some(format!("0x{}", hex::encode(deposit_result.tx_hash)));
        result.deposit_id = Some(format!("0x{}", hex::encode(deposit_result.deposit_id)));

        // 6. Calculate stream info
        let tokens_per_month = (flow_rate as u128) * 30 * 24 * 3600;
        let estimated_end = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() + (wrap_amount_super / flow_rate as u128) as u64;

        result.stream_info = Some(StreamInfo {
            recipient: format!("{}", escrow_extra.recipient),
            flow_rate: escrow_extra.flow_rate.clone(),
            tokens_per_month: tokens_per_month.to_string(),
            estimated_end_time: estimated_end,
            refundable_amount: wrap_amount_super.to_string(),
        });

        result.success = true;
        tracing::info!(
            deposit_id = %result.deposit_id.as_ref().unwrap(),
            buyer = %payer,
            recipient = %escrow_extra.recipient,
            amount = %wrap_amount_super,
            flow_rate = %flow_rate,
            "Superfluid escrow deposit created"
        );

        Ok(result)
    }

    /// Request refund from escrow
    pub async fn request_escrow_refund(
        &self,
        deposit_id: FixedBytes<32>,
        buyer_signature: &str,
    ) -> Result<RefundResponse, SuperfluidError> {
        // Verify signature proves caller is buyer
        // (Implementation depends on EIP-712 domain)

        let escrow_address = self.superfluid_escrow_for_network()?;
        let escrow = SuperfluidEscrow::new(escrow_address, &self.provider);

        // Call requestRefund on escrow contract
        let tx = escrow.requestRefund(deposit_id).send().await?;
        let receipt = tx.get_receipt().await?;

        // Parse RefundProcessed event from logs
        let refunded = parse_refund_event(&receipt.logs)?;

        Ok(RefundResponse {
            success: true,
            transaction: Some(format!("0x{}", hex::encode(receipt.transaction_hash))),
            refunded_amount: Some(refunded.refund_amount.to_string()),
            streamed_amount: Some(refunded.streamed_amount.to_string()),
            error: None,
        })
    }
}
```

#### New HTTP Endpoints

```rust
// Add to src/handlers.rs

/// POST /superfluid/escrow/settle - Settle with escrow-backed streaming
pub async fn settle_superfluid_escrow(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SettleRequest>,
) -> impl IntoResponse {
    // Extract escrow extra from request
    let escrow_extra = match request.payment_requirements.extra.get("superfluidEscrow") {
        Some(extra) => serde_json::from_value::<SuperfluidEscrowExtra>(extra.clone())
            .map_err(|e| SuperfluidError::InvalidPayload(e.to_string()))?,
        None => return (StatusCode::BAD_REQUEST, Json(SuperfluidEscrowResponse {
            success: false,
            error: Some("Missing superfluidEscrow extra".into()),
            ..Default::default()
        })).into_response(),
    };

    match state.facilitator.settle_superfluid_escrow(&request, &escrow_extra).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(SuperfluidEscrowResponse {
            success: false,
            error: Some(e.to_string()),
            ..Default::default()
        })).into_response(),
    }
}

/// POST /superfluid/refund - Request refund for escrowed streaming deposit
pub async fn refund_superfluid_escrow(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RefundEscrowRequest>,
) -> impl IntoResponse {
    let deposit_id = FixedBytes::from_slice(
        &hex::decode(request.deposit_id.trim_start_matches("0x"))
            .unwrap_or_default()
    );

    match state.facilitator.request_escrow_refund(deposit_id, &request.signature).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(RefundResponse {
            success: false,
            error: Some(e.to_string()),
            ..Default::default()
        })).into_response(),
    }
}

/// GET /superfluid/escrow/:network/:deposit_id - Query deposit status
pub async fn get_escrow_deposit(
    State(state): State<Arc<AppState>>,
    Path((network, deposit_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Query escrow contract for deposit details
    // Return current streamed amount, refundable amount, status
}
```

### SuperfluidEscrow Contract Addresses (To Deploy)

```rust
/// SuperfluidEscrow addresses per network
/// Deploy AFTER x402r SessionEscrow is validated
pub mod superfluid_escrow_addresses {
    use super::*;

    // Testnets (deploy first for testing)
    pub const BASE_SEPOLIA: Address = Address::ZERO; // TBD
    pub const OPTIMISM_SEPOLIA: Address = Address::ZERO; // TBD
    pub const AVALANCHE_FUJI: Address = Address::ZERO; // TBD

    // Mainnets (deploy after testnet validation)
    pub const BASE: Address = Address::ZERO; // TBD
    pub const OPTIMISM: Address = Address::ZERO; // TBD
    pub const ARBITRUM: Address = Address::ZERO; // TBD
    pub const POLYGON: Address = Address::ZERO; // TBD
    pub const AVALANCHE: Address = Address::ZERO; // TBD - for UVDx
    pub const ETHEREUM: Address = Address::ZERO; // TBD
    pub const CELO: Address = Address::ZERO; // TBD
}
```

### User Flow Comparison

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         FLOW A: Basic Superfluid                        │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  User pays $50 via x402                                                 │
│       │                                                                 │
│       ├── Facilitator wraps USDC → USDCx                                │
│       ├── Facilitator sends USDCx to User's wallet                      │
│       └── User must manually:                                           │
│           ├── Grant ACL permissions to recipient                        │
│           └── Create stream to recipient                                │
│                                                                         │
│  ❌ User needs 2+ transactions after payment                            │
│  ❌ No automated refund                                                  │
│  ❌ User must manage stream lifecycle                                    │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                    FLOW B: x402r + Superfluid Escrow                    │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  User pays $50 via x402 with escrow extension                           │
│       │                                                                 │
│       ├── Facilitator wraps USDC → USDCx                                │
│       ├── Facilitator deposits USDCx to SuperfluidEscrow                │
│       ├── Escrow creates stream to recipient automatically              │
│       │                                                                 │
│       │   User wants refund after $20 streamed?                         │
│       │   └── POST /superfluid/refund                                   │
│       │       ├── Escrow stops stream                                   │
│       │       └── Escrow returns $30 to user (minus 0.5% fee)           │
│       │                                                                 │
│       └── Stream runs until deposit exhausted or user refunds           │
│                                                                         │
│  ✅ Single payment transaction                                          │
│  ✅ Automated stream creation                                           │
│  ✅ Trustless refunds at any time                                       │
│  ✅ User doesn't manage stream                                          │
│  ✅ Pro-rata refunds based on time streamed                             │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Phase 2 Deployment Steps (When Ready for Escrow)

| Step | Description | Networks | Effort |
|------|-------------|----------|--------|
| **2.1** | Write SuperfluidEscrow.sol + fix 8 security issues | - | 3-5 days |
| **2.2** | Professional security audit (external) | - | 5-7 days |
| **2.3** | Deploy contract to ALL testnets + testing | Base Sepolia, Optimism Sepolia, Fuji, Ethereum Sepolia | 2 days |
| **2.4** | Add escrow endpoints + integration tests | - | 2 days |
| **2.5** | Deploy contract to mainnets (staged rollout) | Ethereum, Base, Polygon, Optimism, Arbitrum, Avalanche, Celo, BSC | 2 days |
| **2.6** | Sessions integration (partial consumption) | All | 3 days |

**Total: 12-15 days** (includes 5-7 days for professional security audit)

### Phase 2 Security Considerations

> **WARNING: DO NOT DEPLOY TO MAINNET WITHOUT PROFESSIONAL AUDIT**
>
> The security-auditor agent identified 3 CRITICAL and 3 HIGH severity vulnerabilities in the Phase 2 contract design. These MUST be fixed and professionally audited before any mainnet deployment.

#### CRITICAL Security Issues (Must Fix Before Development)

| ID | Severity | Issue | Impact | Fix Required |
|----|----------|-------|--------|--------------|
| P2-1 | **CRITICAL** | Streamed amount uses elapsed time, not actual balance | **FUND LOSS** if stream liquidated early | Query actual Super Token balance, not time-based calculation |
| P2-2 | **CRITICAL** | Deposit ID collision in same block | First deposit overwritten, funds lost | Use nonce-based ID: `keccak256(buyer, seller, nonce)` |
| P2-3 | **CRITICAL** | Refund signature verification is TODO | Anyone can trigger unauthorized refunds | Implement EIP-712 signature verification |
| P2-4 | HIGH | Single stream per recipient tracking | Multiple subscriptions from same buyer break | Use `(buyer, seller, depositId)` composite key |
| P2-5 | HIGH | `startStream()` has no access control | Anyone can start streams from escrow | Add `onlyFacilitator` modifier |
| P2-6 | HIGH | Protocol fee charged on refund, not service | Perverse incentives (fee even if service failed) | Fee only on successful service delivery |
| P2-7 | MEDIUM | No emergency pause mechanism | Cannot halt if vulnerability found | Add `Pausable` from OpenZeppelin |
| P2-8 | MEDIUM | No Super Token allowlist | Malicious tokens accepted | Validate against Superfluid TokenFactory |

#### Phase 2 Mitigation Checklist (Before Development)

- [ ] Fix all 8 security issues in contract design document
- [ ] Add comprehensive test coverage for attack vectors
- [ ] Perform internal security review
- [ ] Schedule professional security audit (1-2 weeks)
- [ ] Deploy to testnet and run bug bounty program

#### Basic Security Measures (Already Planned)

1. **Reentrancy**: Contract uses ReentrancyGuard for all state-changing functions
2. **Stream Griefing**: Minimum deposit prevents dust attacks
3. **Liquidation Risk**: Escrow holds Super Tokens, must monitor for low balance
4. **Access Control**: Only facilitator can create deposits, only buyer can refund
5. **Fee Limits**: Protocol fee capped at 5% to prevent abuse
6. **Callback Safety**: Super App callbacks must not revert (would jail the app)

---

## Security Considerations (General)

1. **Decimal Conversion**: Use checked arithmetic to prevent overflow
2. **Fee Validation**: Always calculate fee server-side, never trust client
3. **Compliance**: Extend sanctions screening to stream recipients
4. **Super Token Validation**: Query on-chain to verify super_token is legitimate
5. **ACL Check**: Verify permissions before attempting stream creation

---

## References

- [Superfluid x402-sf GitHub](https://github.com/superfluid-org/x402-sf)
- [Superfluid Documentation](https://docs.superfluid.org/)
- [CFAv1Forwarder Reference](https://docs.superfluid.org/docs/technical-reference/CFAv1Forwarder)
- [UVDx on Superfluid Explorer](https://explorer.superfluid.org/avalanche-c/supertokens/0x11C6AD55Aad69f4612e374e5237b71D580F38f06)
- [UVD on DexScreener](https://dexscreener.com/avalanche/0xbff3e2238e545c76f705560bd1677bd9c0e9dab4)
