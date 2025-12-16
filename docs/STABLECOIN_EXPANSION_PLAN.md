# Stablecoin Expansion Plan for x402-rs Payment Facilitator

**Document Version:** 1.0
**Created:** 2024-12-16
**Status:** Planning
**Author:** Ultravioleta DAO Technical Team

---

## Executive Summary

The x402-rs payment facilitator currently supports only **USDC** (USD Coin by Circle) across 20+ blockchain networks. This document proposes expanding stablecoin support to include additional EIP-3009 compatible stablecoins, enabling broader payment options for users while maintaining the same gasless meta-transaction architecture.

**Key Findings:**
- **6 stablecoins** identified with EIP-3009 support (transferWithAuthorization)
- **USDT, DAI, FRAX explicitly DO NOT support EIP-3009** and cannot be integrated
- **EURC (Euro Coin)** is the highest-priority addition, offering native Euro payments on 3 major chains
- **Estimated implementation effort:** 2-3 weeks for 2-3 additional stablecoins
- **No architectural changes required** - existing EIP-3009 infrastructure reusable
- **Primary benefit:** Multi-currency support (USD, EUR, experimental stablecoins)

---

## Table of Contents

1. [Background: Why Expand Stablecoin Support](#1-background)
2. [EIP-3009 Compatibility Analysis](#2-eip-3009-compatibility-analysis)
3. [Stablecoin Compatibility Matrix](#3-stablecoin-compatibility-matrix)
4. [Detailed Token Profiles](#4-detailed-token-profiles)
5. [Implementation Priority Ranking](#5-implementation-priority-ranking)
6. [Technical Requirements](#6-technical-requirements)
7. [Risk Assessment](#7-risk-assessment)
8. [Implementation Roadmap](#8-implementation-roadmap)
9. [Cost Analysis](#9-cost-analysis)
10. [Testing Requirements](#10-testing-requirements)
11. [Documentation Requirements](#11-documentation-requirements)
12. [Sources and References](#12-sources-and-references)

---

## 1. Background: Why Expand Stablecoin Support

### Current State

The facilitator implements the **x402 HTTP 402 Payment Required protocol**, enabling gasless micropayments across 20+ networks. Currently, only **USDC** is supported, meaning:
- All payments must be denominated in USD
- Users must hold USDC tokens
- No native support for non-USD currencies (EUR, etc.)
- Limited optionality for users preferring other stablecoins

### Strategic Benefits of Expansion

1. **Multi-Currency Support**: Enable Euro-denominated payments via EURC
2. **User Choice**: Support users who prefer alternative stablecoins (GHO, PYUSD, AUSD)
3. **DeFi Integration**: Support DeFi-native stablecoins (GHO, crvUSD)
4. **Geographic Expansion**: EURC adoption strong in European markets
5. **Risk Diversification**: Reduce dependency on single token issuer (Circle)
6. **Competitive Advantage**: First x402 facilitator with multi-stablecoin support

### Why EIP-3009 Matters

The x402 protocol relies on **EIP-3009: Transfer With Authorization** to enable gasless payments:
- Users sign authorization off-chain (EIP-712 signature)
- Facilitator submits transaction on-chain, paying gas
- Transfer happens atomically in single transaction
- No on-chain approvals needed (unlike EIP-2612 permit)

**Only tokens implementing EIP-3009 can be integrated** without architectural changes.

---

## 2. EIP-3009 Compatibility Analysis

### EIP-3009 Compatible Tokens (VERIFIED)

These tokens have **verified EIP-3009 support** through contract verification and documentation:

1. **USDC (USD Coin)** - Circle - ✅ **Currently Supported**
2. **EURC (Euro Coin)** - Circle - ✅ Confirmed
3. **AUSD (Agora USD)** - Agora Finance - ✅ Confirmed
4. **PYUSD (PayPal USD)** - PayPal - ✅ Confirmed
5. **GHO** - Aave - ✅ Confirmed
6. **crvUSD** - Curve Finance - ✅ Confirmed (limited networks)

### Tokens WITHOUT EIP-3009 Support

These major stablecoins **DO NOT** implement EIP-3009 and **CANNOT** be integrated without architectural changes:

- **USDT (Tether)** - Uses custom permit, no EIP-3009 ([source](https://dev.to/extropy/an-overview-of-eip-3009-transfer-with-authorisation-3j50))
- **DAI (MakerDAO)** - Uses EIP-2612 permit only
- **FRAX** - No EIP-3009 support

### Verification Methodology

Contract verification performed via:
1. **Etherscan/block explorer contract source review** - Check for `transferWithAuthorization()` function
2. **Official documentation** - Circle, PayPal, Aave, Curve, Agora docs
3. **Community sources** - Dev.to articles, GitHub discussions
4. **ABI analysis** - Confirm function selector matches EIP-3009 spec

---

## 3. Stablecoin Compatibility Matrix

### Network Deployment Status

Legend:
- `[OK]` = Deployed with verified EIP-3009 support
- `[NO]` = Deployed but no EIP-3009 implementation
- `[??]` = Deployment status unverified (needs research)
- `--` = Not deployed on this network

| Token      | Ethereum | Base   | Polygon | Arbitrum | Optimism | Avalanche | Celo   | Sei    | Unichain | HyperEVM | Monad  |
|------------|----------|--------|---------|----------|----------|-----------|--------|--------|----------|----------|--------|
| **USDC**   | [OK]     | [OK]   | [OK]    | [OK]     | [OK]     | [OK]      | [OK]   | [NO]   | [OK]     | [??]     | [??]   |
| **EURC**   | [OK]     | [OK]   | [??]    | --       | --       | [OK]      | --     | --     | --       | --       | --     |
| **AUSD**   | [OK]     | [OK]   | [OK]    | [OK]     | --       | [OK]      | --     | --     | --       | --       | --     |
| **PYUSD**  | [OK]     | --     | --      | --       | --       | --        | --     | --     | --       | --       | --     |
| **GHO**    | [OK]     | [OK]   | --      | [OK]     | --       | --        | --     | --     | --       | --       | --     |
| **crvUSD** | [OK]     | [NO]   | [NO]    | [OK]     | [NO]     | --        | --     | --     | --       | --       | --     |

### Notes on Matrix

- **EURC** has strong adoption on Ethereum, Base, and Avalanche (priority networks)
- **AUSD** uses deterministic CREATE2 deployment (`0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a`) across all chains
- **PYUSD** limited to Ethereum mainnet only (as of Dec 2024)
- **GHO** focused on Aave ecosystems (Ethereum, Arbitrum, Base)
- **crvUSD** has mixed EIP-3009 support across chains (needs per-chain verification)
- **Sei**: USDC deployed but no EIP-3009 support confirmed

---

## 4. Detailed Token Profiles

### 4.1 EURC (Euro Coin) - Circle

**Status:** ✅ High Priority
**Issuer:** Circle (same as USDC)
**Peg:** 1 EURC = 1 EUR
**Total Supply:** ~$100M+ (as of Q4 2024)

#### Deployment Addresses

| Network          | Address                                      | Decimals | EIP-712 Domain              |
|------------------|----------------------------------------------|----------|-----------------------------|
| Ethereum Mainnet | `0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c` | 6        | name: "Euro Coin", version: "2" |
| Base Mainnet     | `0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42` | 6        | name: "Euro Coin", version: "2" |
| Avalanche Mainnet| `0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD` | 6        | name: "Euro Coin", version: "2" |

#### Why Priority #1

- **Same infrastructure as USDC** - Circle token, identical ABI
- **Strong market fit** - European users prefer Euro denomination
- **Production-ready** - Mature token with regulatory backing
- **Low risk** - Circle reputation, audited contracts
- **Easy integration** - Copy-paste USDC implementation, change addresses

#### Testnet Availability

- **Ethereum Sepolia:** Available (to be verified)
- **Base Sepolia:** Available (to be verified)
- **Avalanche Fuji:** Available (to be verified)

#### Sources

- [Circle EURC](https://www.circle.com/eurc)
- [Circle Developer Docs](https://developers.circle.com/docs/eurc-on-main-networks)

---

### 4.2 AUSD (Agora USD) - Agora Finance

**Status:** ✅ Medium-High Priority
**Issuer:** Agora Finance (backed by VanEck, Dragonfly Capital)
**Peg:** 1 AUSD = 1 USD
**Total Supply:** ~$50M+ (as of Q4 2024)

#### Deployment Addresses

**Deterministic CREATE2 address across all chains:**
- **All Networks:** `0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a`

| Network          | Chain ID | EIP-712 Domain                      |
|------------------|----------|-------------------------------------|
| Ethereum Mainnet | 1        | name: "AUSD", version: "1" (verify) |
| Base Mainnet     | 8453     | name: "AUSD", version: "1" (verify) |
| Polygon Mainnet  | 137      | name: "AUSD", version: "1" (verify) |
| Arbitrum One     | 42161    | name: "AUSD", version: "1" (verify) |
| Avalanche C-Chain| 43114    | name: "AUSD", version: "1" (verify) |

#### Why Interesting

- **Institutional backing** - VanEck (major asset manager) co-issuer
- **Multi-chain native** - Same address everywhere (easier integration)
- **Competitive APY** - Offers yield to holders via RWA backing
- **Growing adoption** - DeFi integration increasing
- **EIP-3009 compliant** - Confirmed in documentation

#### Risk Considerations

- **Newer token** - Less battle-tested than USDC/EURC
- **Smaller market cap** - Lower liquidity than Circle stablecoins
- **Regulatory uncertainty** - Less regulatory clarity than Circle

#### Testnet Availability

- **Ethereum Sepolia:** To be verified
- **Base Sepolia:** To be verified
- **Other testnets:** Unknown

#### Sources

- [Agora AUSD](https://www.agora.finance/)
- [Agora Docs](https://docs.agora.finance)

---

### 4.3 PYUSD (PayPal USD) - PayPal

**Status:** ⚠️ Medium Priority (Limited Network Support)
**Issuer:** PayPal (via Paxos)
**Peg:** 1 PYUSD = 1 USD
**Total Supply:** ~$500M+ (as of Q4 2024)

#### Deployment Addresses

| Network          | Address                                      | Decimals | EIP-712 Domain                        |
|------------------|----------------------------------------------|----------|---------------------------------------|
| Ethereum Mainnet | `0x6c3ea9036406852006290770BEdFcAbA0e23A0e8` | 6        | name: "PayPal USD", version: "1" (verify) |

**Note:** Solana deployment exists but uses SPL Token standard (not EIP-3009)

#### Why Interesting

- **Brand recognition** - PayPal name brings mainstream credibility
- **Large total supply** - Significant adoption in PayPal ecosystem
- **EIP-3009 support** - Confirmed in contract
- **Institutional use** - PayPal integrated into checkout flow

#### Limitations

- **Ethereum only** - No L2 or sidechain deployments yet
- **Ethereum gas costs** - Expensive for micropayments on mainnet
- **Limited DeFi integration** - Less integrated than USDC/DAI

#### Risk Considerations

- **Single network** - Limits utility for multi-chain facilitator
- **Centralization** - PayPal controls issuance and redemption
- **Regulatory risk** - PayPal subject to US regulations

#### Testnet Availability

- **Ethereum Sepolia:** Unknown (likely unavailable)

#### Sources

- [PayPal PYUSD Announcement](https://newsroom.paypal-corp.com/2023-08-07-PayPal-Launches-U-S-Dollar-Stablecoin)
- [Etherscan Contract](https://etherscan.io/token/0x6c3ea9036406852006290770BEdFcAbA0e23A0e8)

---

### 4.4 GHO - Aave

**Status:** ⚠️ Low-Medium Priority (DeFi Native)
**Issuer:** Aave DAO
**Peg:** 1 GHO = 1 USD (soft peg via Aave mechanisms)
**Total Supply:** ~$50M+ (as of Q4 2024)

#### Deployment Addresses

| Network          | Address                                      | Decimals | EIP-712 Domain                  |
|------------------|----------------------------------------------|----------|---------------------------------|
| Ethereum Mainnet | `0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f` | 18       | name: "Gho Token", version: "1" |
| Arbitrum One     | `0x7dfF72693f6A4149b17e7C6314655f6A9F7c8B33` | 18       | name: "Gho Token", version: "1" |
| Base Mainnet     | `0x6Bb7a212910682DCFdbd5BCBb3e28FB4E8da10Ee` | 18       | name: "Gho Token", version: "1" |

#### Why Interesting

- **DeFi native** - Created by Aave, one of largest DeFi protocols
- **Decentralized issuance** - Minted via borrowing against collateral
- **EIP-3009 support** - Confirmed in contract
- **Growing adoption** - Integrated in Aave ecosystem

#### Limitations

- **18 decimals** - Different from 6-decimal USDC (requires UI changes)
- **Soft peg** - Not 1:1 fiat-backed (price can fluctuate slightly)
- **Limited networks** - Only Ethereum, Arbitrum, Base
- **Smaller liquidity** - Less liquid than USDC

#### Risk Considerations

- **Peg stability risk** - Algorithmic peg, not fiat-backed
- **Smart contract risk** - More complex minting mechanism
- **Lower adoption** - Less used than fiat-backed stablecoins

#### Testnet Availability

- **Ethereum Sepolia:** Available (Aave testnet)
- **Arbitrum Sepolia:** Likely available
- **Base Sepolia:** Unknown

#### Sources

- [Aave GHO](https://aave.com/gho)
- [GHO Documentation](https://docs.aave.com/faq/gho-stablecoin)

---

### 4.5 crvUSD - Curve Finance

**Status:** ⚠️ Low Priority (Inconsistent Support)
**Issuer:** Curve DAO
**Peg:** 1 crvUSD = 1 USD (via LLAMMA mechanism)
**Total Supply:** ~$100M+ (as of Q4 2024)

#### Deployment Addresses

| Network          | Address                                      | Decimals | EIP-712 Support | EIP-3009 Status |
|------------------|----------------------------------------------|----------|-----------------|-----------------|
| Ethereum Mainnet | `0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E` | 18       | ✅ Yes          | ✅ [OK]         |
| Arbitrum One     | `0x498Bf2B1e120FeD3ad3D42EA2165E9b73f99C1e5` | 18       | ✅ Yes          | ✅ [OK]         |
| Base Mainnet     | [Deployed]                                   | 18       | Unknown         | [NO]            |
| Polygon Mainnet  | [Deployed]                                   | 18       | Unknown         | [NO]            |
| Optimism Mainnet | [Deployed]                                   | 18       | Unknown         | [NO]            |

#### Why Limited Priority

- **Inconsistent EIP-3009 support** - Not all deployments have transferWithAuthorization
- **18 decimals** - Different from 6-decimal USDC
- **Soft peg** - LLAMMA mechanism can cause temporary depeg
- **Complexity** - Harder to verify contract support per chain
- **Lower adoption** - Primarily used within Curve ecosystem

#### Risk Considerations

- **Peg stability risk** - Algorithmic stabilization mechanism
- **Smart contract risk** - Complex LLAMMA system
- **Verification burden** - Need to verify each deployment individually

#### Recommendation

**DEFER** until after EURC, AUSD implementation. Only consider for Ethereum and Arbitrum initially.

#### Sources

- [Curve crvUSD](https://curve.fi)
- [crvUSD Documentation](https://docs.curve.fi/crvusd/)

---

## 5. Implementation Priority Ranking

### Tier 1: High Priority (Implement First)

#### 1. EURC (Euro Coin) - Score: 95/100

**Rationale:**
- Identical implementation to USDC (Circle infrastructure)
- Strong market need (Euro payments)
- Low technical risk
- Available on 3 major networks (Ethereum, Base, Avalanche)
- Regulatory clarity (Circle regulated entity)

**Estimated effort:** 1 week
**Networks to support:** Ethereum, Base, Avalanche (mainnet + testnet)

---

### Tier 2: Medium Priority (Implement After EURC)

#### 2. AUSD (Agora USD) - Score: 75/100

**Rationale:**
- Institutional backing (VanEck)
- Multi-chain deployment (5+ networks)
- Same address everywhere (simpler integration)
- Growing adoption in DeFi
- EIP-3009 confirmed

**Estimated effort:** 1 week
**Networks to support:** Ethereum, Base, Arbitrum, Polygon, Avalanche

---

### Tier 3: Lower Priority (Evaluate After Phase 1-2)

#### 3. GHO (Aave) - Score: 60/100

**Rationale:**
- DeFi-native audience
- 18 decimals (different UX consideration)
- Smaller user base
- Limited to 3 networks
- Soft peg (algorithmic)

**Estimated effort:** 1.5 weeks (18-decimal handling)
**Networks to support:** Ethereum, Arbitrum, Base

#### 4. PYUSD (PayPal USD) - Score: 55/100

**Rationale:**
- Strong brand recognition
- Ethereum only (major limitation)
- High mainnet gas costs
- Unknown testnet availability

**Estimated effort:** 3-5 days (single network)
**Networks to support:** Ethereum mainnet only

#### 5. crvUSD (Curve) - Score: 45/100

**Rationale:**
- Inconsistent EIP-3009 support across chains
- High verification burden
- 18 decimals
- Niche audience (Curve users)
- Algorithmic peg risk

**Estimated effort:** 2 weeks (per-chain verification + testing)
**Networks to support:** Ethereum, Arbitrum only (confirmed EIP-3009)

---

## 6. Technical Requirements

### 6.1 Code Changes Required

For each new stablecoin, the following changes are needed:

#### Backend (Rust)

**File: `src/network.rs`**
```rust
// Add token enum (example for EURC)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenType {
    USDC,
    EURC,  // NEW
    AUSD,  // NEW
    // ... etc
}

// Add deployment constants (per network)
static EURC_ETHEREUM: Lazy<TokenDeployment> = Lazy::new(|| {
    TokenDeployment {
        asset: TokenAsset {
            address: address!("0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c").into(),
            network: Network::Ethereum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "Euro Coin".into(),
            version: "2".into(),
        }),
    }
});

// Repeat for each network (Base, Avalanche, etc.)
```

**File: `src/types.rs`**
```rust
// Add token type to TokenAsset
pub struct TokenAsset {
    pub address: MixedAddress,
    pub network: Network,
    pub token_type: TokenType,  // NEW FIELD
}

// Update PaymentPayload to support token selection
pub struct PaymentPayload {
    pub network: Network,
    pub token: TokenType,  // NEW: Allow client to specify token
    // ... existing fields
}
```

**File: `src/handlers.rs`**
```rust
// Update /supported endpoint to list tokens per network
pub async fn get_supported() -> Result<Json<SupportedPaymentKindsResponse>, FacilitatorError> {
    // Return multiple entries per network (one per token)
    // Example: base-mainnet-usdc, base-mainnet-eurc, etc.
}
```

**File: `src/facilitator_local.rs`**
```rust
// Update verify/settle to look up token deployment
impl FacilitatorLocal {
    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Error> {
        let deployment = match request.payment_payload.token {
            TokenType::USDC => USDCDeployment::by_network(network),
            TokenType::EURC => EURCDeployment::by_network(network),
            TokenType::AUSD => AUSDDeployment::by_network(network),
            // ...
        };
        // ... existing verification logic
    }
}
```

#### Frontend (HTML/JavaScript)

**File: `static/index.html`**

Add token balance display per network:

```html
<!-- Example: Base mainnet card -->
<div class="network-badge base">
    <div>Base Mainnet</div>
    <div>
        <span data-balance="base-mainnet-usdc">USDC: Loading...</span>
        <span data-balance="base-mainnet-eurc">EURC: Loading...</span>
    </div>
</div>
```

Update JavaScript to query multiple token balances:

```javascript
const TOKEN_ABIS = {
    erc20: [/* ERC-20 balanceOf ABI */],
};

async function loadTokenBalance(network, token, address) {
    const config = BALANCE_CONFIG[`${network}-${token}`];
    // ... load balance via RPC
}
```

#### Estimated Code Changes

| Component            | Lines Changed | New Files | Modified Files |
|----------------------|---------------|-----------|----------------|
| Backend (Rust)       | ~300-500      | 0         | 4-5            |
| Frontend (HTML/JS)   | ~100-200      | 0         | 1              |
| Tests (Python)       | ~100-150      | 2-3       | 2-3            |
| Documentation        | ~50-100       | 1         | 2-3            |
| **TOTAL**            | **550-950**   | **3-4**   | **9-12**       |

---

### 6.2 Infrastructure Changes

#### AWS Secrets Manager

No changes required - existing wallet keys work for all EIP-3009 tokens.

#### Environment Variables

No new RPC endpoints needed - existing network RPCs support all tokens.

#### Docker Image

- Rebuild with updated code
- Image size increase: Negligible (~1-2 MB for additional ABIs)

---

### 6.3 Protocol Changes

#### x402 Protocol Extension (Optional)

Current PaymentPayload structure:
```json
{
  "network": "base-mainnet",
  "scheme": "exact",
  "amount": "1000000",
  // ... existing fields
}
```

Proposed extension:
```json
{
  "network": "base-mainnet",
  "token": "eurc",  // NEW: Token type
  "scheme": "exact",
  "amount": "1000000",
  // ... existing fields
}
```

**Backward Compatibility:**
- Default to "usdc" if `token` field omitted
- Existing v1 clients continue working unchanged

---

## 7. Risk Assessment

### 7.1 Technical Risks

| Risk                          | Severity | Likelihood | Mitigation                                      |
|-------------------------------|----------|------------|-------------------------------------------------|
| EIP-3009 implementation bugs  | High     | Low        | Thorough contract verification, testnet testing |
| 18-decimal token handling     | Medium   | Medium     | Decimal-aware amount parsing, UI warnings       |
| EIP-712 domain mismatch       | Medium   | Low        | Verify domain per token via contract call       |
| Testnet token unavailability  | Low      | Medium     | Mainnet-first deployment, synthetic test tokens |
| Token contract upgrades       | Medium   | Low        | Monitor token issuer announcements              |

### 7.2 Operational Risks

| Risk                          | Severity | Likelihood | Mitigation                                      |
|-------------------------------|----------|------------|-------------------------------------------------|
| Low token liquidity           | Medium   | Medium     | Only support high-liquidity tokens (EURC, AUSD) |
| Token depeg events            | High     | Low        | Focus on fiat-backed tokens, monitor peg health |
| Regulatory changes            | Medium   | Medium     | Support Circle tokens (regulated), monitor news |
| Wallet funding complexity     | Low      | Low        | Native tokens (ETH/AVAX) still cover gas        |

### 7.3 Security Risks

| Risk                          | Severity | Likelihood | Mitigation                                      |
|-------------------------------|----------|------------|-------------------------------------------------|
| Malicious token contracts     | High     | Very Low   | Only support verified, reputable tokens         |
| Token supply manipulation     | Medium   | Very Low   | Fiat-backed tokens have audited reserves        |
| Smart contract bugs           | High     | Low        | Use battle-tested tokens (Circle, Aave)         |
| Phishing attacks              | Medium   | Medium     | Clear UI indicating token type, contract verify |

### 7.4 Market Risks

| Risk                          | Severity | Likelihood | Mitigation                                      |
|-------------------------------|----------|------------|-------------------------------------------------|
| Low user adoption             | Low      | Medium     | Prioritize EURC (strong EU demand)              |
| Token discontinuation         | Medium   | Very Low   | Only support major issuers (Circle, PayPal)     |
| Competitive pressure          | Low      | Low        | Multi-stablecoin support is differentiator      |

### Overall Risk Score: **Low-Medium**

The primary risks are **operational** (liquidity, adoption) rather than technical. Implementation risk is low because:
- EIP-3009 infrastructure already proven with USDC
- Circle tokens (EURC) nearly identical to existing USDC implementation
- No fundamental architecture changes required

---

## 8. Implementation Roadmap

### Phase 1: EURC Integration (Weeks 1-2)

**Goal:** Add Euro-denominated payment support via Circle EURC

**Week 1: Backend Implementation**
- [ ] Add `TokenType` enum to `src/network.rs`
- [ ] Add EURC deployment constants for Ethereum, Base, Avalanche
- [ ] Update `TokenAsset` struct to include token type
- [ ] Modify `/supported` endpoint to list tokens per network
- [ ] Update verification logic to select token deployment
- [ ] Add EURC testnet addresses (if available)
- [ ] Unit tests for token selection logic

**Week 2: Frontend, Testing, Deployment**
- [ ] Update landing page to show EURC balances
- [ ] Add EURC balance loading JavaScript
- [ ] Python integration tests for EURC payments
- [ ] Testnet end-to-end testing (if available)
- [ ] Documentation updates (CHANGELOG, CLAUDE.md)
- [ ] Build Docker image v1.9.0
- [ ] Deploy to production
- [ ] Verify `/supported` includes EURC networks
- [ ] Mainnet smoke testing

**Deliverables:**
- EURC support on 3 networks (Ethereum, Base, Avalanche)
- Updated frontend showing EURC balances
- Integration tests for EURC
- Documentation updates

---

### Phase 2: AUSD Integration (Weeks 3-4)

**Goal:** Add Agora USD support across 5+ networks

**Week 3: Backend Implementation**
- [ ] Add AUSD deployment constants (deterministic address)
- [ ] Verify EIP-712 domain info per network
- [ ] Update `/supported` endpoint
- [ ] Add AUSD to verification/settlement logic
- [ ] Unit tests for AUSD

**Week 4: Frontend, Testing, Deployment**
- [ ] Update landing page for AUSD balances
- [ ] Integration tests for AUSD
- [ ] Testnet testing (if available)
- [ ] Build Docker image v1.10.0
- [ ] Deploy to production
- [ ] Verify AUSD on 5 networks

**Deliverables:**
- AUSD support on 5 networks (Ethereum, Base, Arbitrum, Polygon, Avalanche)
- Frontend AUSD balance display
- Integration tests
- Documentation

---

### Phase 3: Evaluation and Expansion (Week 5+)

**Goal:** Assess Phase 1-2 success, decide on Phase 3 tokens

**Week 5: Analytics and Decision**
- [ ] Analyze EURC usage metrics (transaction count, volume)
- [ ] Analyze AUSD usage metrics
- [ ] User feedback collection
- [ ] Decide on Phase 3 tokens (GHO, PYUSD, crvUSD, or pause)

**If expanding to GHO:**
- Week 6-7: GHO integration (3 networks)
- Requires 18-decimal handling in frontend
- Documentation for decimal differences

**If expanding to PYUSD:**
- Week 6: PYUSD integration (Ethereum only)
- Simple integration (single network)
- Marketing value (PayPal brand)

**If expanding to crvUSD:**
- Week 6-8: crvUSD integration (Ethereum, Arbitrum)
- Requires per-chain EIP-3009 verification
- 18-decimal handling

---

### Milestones

| Milestone                  | Target Date    | Deliverables                              |
|----------------------------|----------------|-------------------------------------------|
| M1: EURC Launch            | Week 2 End     | EURC on 3 networks, production deployed   |
| M2: AUSD Launch            | Week 4 End     | AUSD on 5 networks, production deployed   |
| M3: Analytics Review       | Week 5 End     | Usage report, Phase 3 decision            |
| M4: Phase 3 Launch (TBD)   | Week 7-8 End   | Additional token(s) if justified          |

---

## 9. Cost Analysis

### 9.1 Development Costs

**Internal effort (Ultravioleta DAO team):**

| Phase        | Developer Hours | Hourly Rate | Total Cost  |
|--------------|-----------------|-------------|-------------|
| Phase 1      | 40-60 hours     | N/A         | Internal    |
| Phase 2      | 40-60 hours     | N/A         | Internal    |
| Phase 3      | 40-80 hours     | N/A         | Internal    |
| **TOTAL**    | **120-200 hrs** | N/A         | **Internal**|

**Note:** Assumes internal team capacity. Outsourcing at $100-150/hr would cost $12,000-30,000.

### 9.2 Infrastructure Costs

**AWS cost impact:** ~$0-2/month additional

- No new VPCs, load balancers, or RPC endpoints required
- Existing ECS task handles additional tokens
- Slight increase in CloudWatch logs volume
- No new Secrets Manager secrets needed

### 9.3 Wallet Funding Costs

**Per token, per network:**
- Mainnet wallet: ~0.1 ETH ($200-400) for initial gas
- Testnet wallet: Free (faucet tokens)

**Total funding needed (if supporting EURC + AUSD):**
- EURC: 3 networks × $300 = $900
- AUSD: 5 networks × $300 = $1,500
- **TOTAL: ~$2,400** (one-time, reusable across many transactions)

### 9.4 Ongoing Operational Costs

**Per month:**
- AWS infrastructure: ~$43-48 (unchanged)
- Monitoring/logs: ~$5-10 (slight increase)
- Transaction gas (users reimburse): $0
- **TOTAL: ~$48-58/month** (~$5-10/month increase)

### Total Cost Estimate

- **One-time:** $2,400 (wallet funding)
- **Monthly:** $5-10 additional operational costs
- **Development:** Internal team effort (~120-200 hours over 2-3 months)

---

## 10. Testing Requirements

### 10.1 Unit Tests (Rust)

**New test files:**
- `tests/unit/token_selection.rs` - Token enum parsing and selection
- `tests/unit/eurc_deployments.rs` - EURC contract addresses
- `tests/unit/ausd_deployments.rs` - AUSD contract addresses

**Test coverage:**
- Token type serialization/deserialization
- Token deployment lookup by network
- EIP-712 domain info per token
- Fallback to USDC if token unspecified (backward compat)

**Target:** 100% coverage for new token logic

---

### 10.2 Integration Tests (Python)

**New test files:**
- `tests/integration/test_eurc_payment.py`
- `tests/integration/test_ausd_payment.py`
- `tests/integration/test_multi_token.py`

**Test scenarios:**
1. **EURC payment flow:**
   - Verify EURC payment authorization (Ethereum, Base, Avalanche)
   - Settle EURC payment on-chain
   - Verify transaction receipt

2. **AUSD payment flow:**
   - Verify AUSD payment on 5 networks
   - Test deterministic address consistency

3. **Multi-token mixing:**
   - Verify USDC payment on Base, EURC payment on Ethereum in same session
   - Ensure no token/network cross-contamination

4. **Backward compatibility:**
   - Verify old clients (no `token` field) default to USDC

**Test environments:**
- Testnets (if available)
- Mainnet fork (Hardhat/Anvil)
- Production (smoke testing only)

---

### 10.3 Frontend Testing

**Manual testing checklist:**
- [ ] EURC balance loads correctly on Ethereum, Base, Avalanche cards
- [ ] AUSD balance loads correctly on 5 network cards
- [ ] Logo display (if adding token logos)
- [ ] Decimal formatting (6-decimal vs 18-decimal)
- [ ] Currency symbols displayed correctly (€ for EURC, $ for USDC/AUSD)

**Automated testing:**
- Playwright end-to-end tests for balance loading
- Screenshot comparison for UI consistency

---

### 10.4 Testnet Verification

**Pre-deployment checklist (per token, per network):**
1. [ ] Testnet token contract verified on block explorer
2. [ ] Testnet facilitator wallet funded with gas
3. [ ] Test user wallet funded with test tokens
4. [ ] Python integration test passes on testnet
5. [ ] `/verify` endpoint succeeds
6. [ ] `/settle` endpoint succeeds
7. [ ] Transaction confirmed on testnet block explorer

---

## 11. Documentation Requirements

### 11.1 User-Facing Documentation

**File: `docs/SUPPORTED_STABLECOINS.md` (NEW)**

Content:
- List of supported stablecoins per network
- Decimal precision per token (6 vs 18)
- How to specify token in payment request
- Currency symbols and denominations
- Links to token issuer websites

**File: `static/index.html`**

Update:
- Landing page FAQ section explaining multi-token support
- Token selection in API example code
- Currency information per network card

---

### 11.2 Developer Documentation

**File: `CLAUDE.md`**

Update sections:
- Supported tokens matrix (add EURC, AUSD columns)
- Configuration: Token deployment addresses
- Example payment requests with token field

**File: `docs/CHANGELOG.md`**

Add release notes:
```markdown
## [1.9.0] - 2025-01-XX

### Added
- EURC (Euro Coin) support on Ethereum, Base, Avalanche
- Multi-token selection in payment payload
- Frontend token balance display

## [1.10.0] - 2025-02-XX

### Added
- AUSD (Agora USD) support on 5 networks
- Updated /supported endpoint with token types
```

**File: `docs/API_REFERENCE.md` (NEW or UPDATE)**

Document extended PaymentPayload:
```json
{
  "network": "base-mainnet",
  "token": "eurc",  // NEW FIELD
  "scheme": "exact",
  "amount": "1000000",
  // ... existing fields
}
```

---

### 11.3 Internal Documentation

**File: `docs/TOKEN_VERIFICATION_CHECKLIST.md` (NEW)**

Checklist for adding new tokens:
- [ ] Verify EIP-3009 support in contract source
- [ ] Verify EIP-712 domain (name/version)
- [ ] Add deployment addresses to `src/network.rs`
- [ ] Add unit tests
- [ ] Add integration tests
- [ ] Fund testnet wallets
- [ ] Fund mainnet wallets
- [ ] Test on testnet
- [ ] Deploy to production
- [ ] Verify on mainnet

---

## 12. Sources and References

### Token Issuer Documentation

1. **USDC (Circle)**
   - Homepage: https://www.circle.com/usdc
   - Developers: https://developers.circle.com/docs/usdc-on-main-networks
   - Contract Source: Verified on Etherscan

2. **EURC (Circle)**
   - Homepage: https://www.circle.com/eurc
   - Developers: https://developers.circle.com/docs/eurc-on-main-networks
   - Contract Source: Verified on Etherscan

3. **AUSD (Agora)**
   - Homepage: https://www.agora.finance/
   - Documentation: https://docs.agora.finance
   - Contract: 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a (all chains)

4. **PYUSD (PayPal)**
   - Announcement: https://newsroom.paypal-corp.com/2023-08-07-PayPal-Launches-U-S-Dollar-Stablecoin
   - Solana Expansion: https://newsroom.paypal-corp.com/2024-05-29-PayPal-USD-Stablecoin-Now-Available-on-Solana-Blockchain
   - Contract: 0x6c3ea9036406852006290770BEdFcAbA0e23A0e8 (Ethereum)

5. **GHO (Aave)**
   - Homepage: https://aave.com/gho
   - Documentation: https://docs.aave.com/faq/gho-stablecoin
   - Governance Forum: https://governance.aave.com/t/gho/

6. **crvUSD (Curve)**
   - Homepage: https://curve.fi
   - Documentation: https://docs.curve.fi/crvusd/
   - Contract Source: Verified on Etherscan

### EIP-3009 Standard

7. **EIP-3009: Transfer With Authorization**
   - Official Spec: https://eips.ethereum.org/EIPS/eip-3009
   - Dev.to Overview: https://dev.to/extropy/an-overview-of-eip-3009-transfer-with-authorisation-3j50
   - GitHub Discussion: https://github.com/ethereum/EIPs/issues/3009

### Non-EIP-3009 Tokens (Excluded)

8. **USDT (Tether)**
   - Why excluded: Custom permit, no EIP-3009 support
   - Source: https://dev.to/extropy/an-overview-of-eip-3009-transfer-with-authorisation-3j50

9. **DAI (MakerDAO)**
   - Why excluded: Only supports EIP-2612 permit (not EIP-3009)
   - Permit Documentation: https://docs.makerdao.com/smart-contract-modules/dai-module

10. **FRAX**
    - Why excluded: No EIP-3009 implementation

### Block Explorers (Contract Verification)

11. **Etherscan** - https://etherscan.io
12. **Basescan** - https://basescan.org
13. **Arbiscan** - https://arbiscan.io
14. **PolygonScan** - https://polygonscan.com
15. **Snowtrace (Avalanche)** - https://snowtrace.io

---

## Appendix A: Contract Addresses Reference

### USDC (Current Support)

| Network            | Address                                      | Chain ID |
|--------------------|----------------------------------------------|----------|
| Ethereum           | 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48   | 1        |
| Base               | 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913   | 8453     |
| Polygon            | 0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359   | 137      |
| Arbitrum           | 0xaf88d065e77c8cC2239327C5EDb3A432268e5831   | 42161    |
| Optimism           | 0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85   | 10       |
| Avalanche          | 0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E   | 43114    |
| Celo               | 0xcebA9300f2b948710d2653dD7B07f33A8B32118C   | 42220    |
| Unichain           | 0x078D782b760474a361dDA0AF3839290b0EF57AD6   | 130      |

### EURC (Proposed)

| Network            | Address                                      | Chain ID |
|--------------------|----------------------------------------------|----------|
| Ethereum           | 0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c   | 1        |
| Base               | 0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42   | 8453     |
| Avalanche          | 0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD   | 43114    |

### AUSD (Proposed)

| Network            | Address (Same on All Chains)                 | Chain ID |
|--------------------|----------------------------------------------|----------|
| Ethereum           | 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a   | 1        |
| Base               | 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a   | 8453     |
| Polygon            | 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a   | 137      |
| Arbitrum           | 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a   | 42161    |
| Avalanche          | 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a   | 43114    |

### PYUSD (Proposed)

| Network            | Address                                      | Chain ID |
|--------------------|----------------------------------------------|----------|
| Ethereum           | 0x6c3ea9036406852006290770BEdFcAbA0e23A0e8   | 1        |

### GHO (Proposed)

| Network            | Address                                      | Chain ID |
|--------------------|----------------------------------------------|----------|
| Ethereum           | 0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f   | 1        |
| Arbitrum           | 0x7dfF72693f6A4149b17e7C6314655f6A9F7c8B33   | 42161    |
| Base               | 0x6Bb7a212910682DCFdbd5BCBb3e28FB4E8da10Ee   | 8453     |

### crvUSD (Proposed - Verified EIP-3009 Only)

| Network            | Address                                      | Chain ID | EIP-3009 Status |
|--------------------|----------------------------------------------|----------|-----------------|
| Ethereum           | 0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E   | 1        | ✅ Verified     |
| Arbitrum           | 0x498Bf2B1e120FeD3ad3D42EA2165E9b73f99C1e5   | 42161    | ✅ Verified     |

---

## Appendix B: EIP-712 Domain Information

| Token  | Network   | Domain Name         | Domain Version | Verifying Contract                           |
|--------|-----------|---------------------|----------------|----------------------------------------------|
| USDC   | Ethereum  | "USD Coin"          | "2"            | 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48   |
| USDC   | Base      | "USD Coin"          | "2"            | 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913   |
| EURC   | Ethereum  | "Euro Coin"         | "2"            | 0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c   |
| EURC   | Base      | "Euro Coin"         | "2"            | 0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42   |
| AUSD   | All       | "AUSD" (verify)     | "1" (verify)   | 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a   |
| PYUSD  | Ethereum  | "PayPal USD" (verify) | "1" (verify) | 0x6c3ea9036406852006290770BEdFcAbA0e23A0e8   |
| GHO    | Ethereum  | "Gho Token"         | "1"            | 0x40D16FC0246aD3160Ccc09B8D0D3A2cD28aE6C2f   |
| crvUSD | Ethereum  | "crvUSD" (verify)   | "1" (verify)   | 0xf939E0A03FB07F59A73314E73794Be0E57ac1b4E   |

**Note:** "verify" indicates domain info needs confirmation via contract call before integration.

---

## Appendix C: Decimal Handling Comparison

| Token  | Decimals | Amount Format Example | Display Example | Notes                              |
|--------|----------|-----------------------|-----------------|------------------------------------|
| USDC   | 6        | "1000000"             | $1.00           | Standard (most stablecoins)        |
| EURC   | 6        | "1000000"             | €1.00           | Same as USDC                       |
| AUSD   | 6        | "1000000"             | $1.00           | Same as USDC                       |
| PYUSD  | 6        | "1000000"             | $1.00           | Same as USDC                       |
| GHO    | 18       | "1000000000000000000" | $1.00           | Requires special UI handling       |
| crvUSD | 18       | "1000000000000000000" | $1.00           | Requires special UI handling       |

**Implementation note:** 18-decimal tokens require:
- Backend: Correct decimal conversion in amount parsing
- Frontend: Clear warnings to users about decimal precision
- Testing: Verify no loss of precision in calculations

---

## Appendix D: Recommended Testing Networks

### Priority 1: Ethereum Mainnet
- Highest liquidity for all tokens
- Most mature deployments
- Essential for PYUSD (Ethereum-only)

### Priority 2: Base Mainnet
- Strong USDC/EURC adoption
- Low gas costs (L2)
- Growing DeFi ecosystem
- Agora AUSD deployed

### Priority 3: Avalanche C-Chain
- USDC/EURC both deployed
- AUSD deployed
- Fast finality
- Active ecosystem

### Priority 4: Arbitrum One
- GHO deployed
- crvUSD deployed
- Large DeFi ecosystem

### Priority 5: Polygon
- AUSD deployed
- High transaction volume
- Cost-effective testing

---

## Conclusion

This stablecoin expansion plan provides a structured approach to adding multi-token support to the x402-rs payment facilitator. The **phased implementation** prioritizes:

1. **EURC** (Tier 1) - Lowest risk, highest market fit, identical to USDC technically
2. **AUSD** (Tier 2) - Strong institutional backing, multi-chain native
3. **GHO, PYUSD, crvUSD** (Tier 3) - Evaluate after Phase 1-2 success

**Key Success Metrics:**
- EURC transaction volume > 10% of USDC volume within 3 months
- User feedback on multi-currency support
- No security incidents or token contract issues
- Smooth technical implementation (no major bugs)

**Go/No-Go Decision Point:** After Phase 2 (EURC + AUSD), assess adoption metrics before committing to Phase 3 tokens.

---

**Document Prepared By:** Ultravioleta DAO Technical Team
**Review Status:** Draft for internal review
**Next Steps:**
1. Internal team review and feedback
2. Prioritize EURC vs AUSD for Phase 1
3. Allocate development resources
4. Begin Phase 1 implementation planning

---

*End of Document*
