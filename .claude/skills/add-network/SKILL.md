---
name: add-network
description: Add new blockchain networks to the x402-rs facilitator. This skill should be used when adding support for a new EVM or Solana network (e.g., "add facilitator scroll", "add network monad"). It performs automated research, gathers USDC contract info, verifies EIP-3009 support, checks wallet balances, and guides through implementation. If all prerequisites are met (logo exists, wallets funded), it can deploy automatically.
---

# Add Network Skill

This skill provides a complete automated workflow for adding new blockchain networks to the x402-rs payment facilitator.

## When to Use This Skill

Invoke this skill when:
- Adding a new EVM chain (Scroll, Monad, Linea, zkSync, etc.)
- Adding a new L2/L3 network
- User says "add facilitator {network}" or "add network {network}"

## Quick Reference: What the Skill Does

```
User: "add facilitator scroll"
         │
         ▼
┌─────────────────────────────────┐
│ 1. RESEARCH PHASE               │
│    - Chain ID, RPCs             │
│    - USDC contracts             │
│    - EIP-3009 verification      │
│    - Explorer URLs              │
│    - Native token name          │
│    - EIP-1559 support           │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 2. PREREQUISITES CHECK          │
│    - Logo exists? (/static/)    │
│    - Mainnet wallet funded?     │
│    - Testnet wallet funded?     │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 3. ASK USER FOR MISSING ITEMS   │
│    - Request PNG if missing     │
│    - Request wallet funding     │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 4. IMPLEMENTATION               │
│    - src/network.rs             │
│    - src/from_env.rs            │
│    - src/chain/evm.rs           │
│    - src/handlers.rs            │
│    - static/index.html          │
│    - .env.example               │
└─────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────┐
│ 5. DEPLOY (if auto-deploy)      │
│    - cargo build                │
│    - docker build & push        │
│    - ECS update                 │
│    - Verification               │
└─────────────────────────────────┘
```

---

## Phase 1: Research

### 1.1 Gather Chain Information

**Required data to collect:**

| Field | Description | Example |
|-------|-------------|---------|
| Network Name | Official name | "Scroll" |
| Mainnet Chain ID | EVM chain ID | 534352 |
| Testnet Chain ID | Testnet chain ID | 534351 |
| Testnet Name | Full testnet name | "Scroll Sepolia" |
| Native Token | Gas token | "ETH" |
| EIP-1559 | Transaction type support | true/false |
| Mainnet RPC | Public RPC URL | https://rpc.scroll.io |
| Testnet RPC | Public RPC URL | https://sepolia-rpc.scroll.io |
| Mainnet Explorer | Block explorer | https://scrollscan.com |
| Testnet Explorer | Block explorer | https://sepolia.scrollscan.com |
| Brand Color | Hex color for CSS | #FFEEDA |

**Research sources:**
1. Official network documentation
2. ChainList.org for chain IDs and RPCs
3. Block explorers for contract verification

### 1.2 Find USDC Contract Addresses

**Search priority:**
1. Circle's official deployments: https://developers.circle.com/stablecoins/docs/usdc-on-main-networks
2. Bridge documentation (canonical USDC vs bridged)
3. Block explorer token search
4. DeFiLlama stablecoin tracker

**For each network, record:**

```
USDC Mainnet:
  - Address: 0x...
  - Decimals: 6
  - Type: Native/Bridged
  - EIP-712 Name: "USD Coin" or "USDC"
  - EIP-712 Version: "2"

USDC Testnet:
  - Address: 0x...
  - Same fields...
```

### 1.3 Verify EIP-3009 Support

**CRITICAL: x402 protocol REQUIRES EIP-3009 `transferWithAuthorization`.**

Run verification for each USDC contract:

```bash
# Test if transferWithAuthorization exists
cast call <USDC_ADDRESS> \
  "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)" \
  0x0000000000000000000000000000000000000001 \
  0x0000000000000000000000000000000000000002 \
  1000000 0 9999999999 \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  0x0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 \
  --rpc-url <RPC_URL>
```

**Interpretation:**
- `"invalid signature"` = EIP-3009 EXISTS (good!)
- `"execution reverted"` (generic) = NOT SUPPORTED (stop here)

### 1.4 Get EIP-712 Domain Metadata

Query the USDC contract to get exact EIP-712 domain:

```bash
# Get name (may differ from token symbol!)
cast call <USDC_ADDRESS> "name()" --rpc-url <RPC> | cast --to-ascii

# Get version
cast call <USDC_ADDRESS> "version()" --rpc-url <RPC> | cast --to-ascii

# Get decimals
cast call <USDC_ADDRESS> "decimals()" --rpc-url <RPC>
```

**IMPORTANT:** EIP-712 name often differs between chains:
- Ethereum/Avalanche: `"USD Coin"`
- Base/Celo/HyperEVM: `"USDC"`
- Some chains: `"Bridged USD Coin"`

Always verify from contract, never assume!

---

## Phase 2: Prerequisites Check

### 2.1 Check Logo Exists

```bash
ls -la static/{network}.png
```

If logo doesn't exist, ask user to provide:
- PNG format
- Transparent background
- ~32x32px or larger
- Place in `static/` directory

### 2.2 Check Wallet Balances

**Facilitator wallet addresses:**
- Mainnet: `0x103040545AC5031A11E8C03dd11324C7333a13C7`
- Testnet: `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`

Check both wallets have native tokens for gas:

```bash
# Mainnet balance
curl -s -X POST <MAINNET_RPC> \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x103040545AC5031A11E8C03dd11324C7333a13C7","latest"],"id":1}' \
  | jq -r '.result' | xargs -I{} python3 -c "print(int('{}', 16) / 1e18)"

# Testnet balance
curl -s -X POST <TESTNET_RPC> \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8","latest"],"id":1}' \
  | jq -r '.result' | xargs -I{} python3 -c "print(int('{}', 16) / 1e18)"
```

**Minimum recommended balances:**
- Mainnet: ~0.01 ETH equivalent (for ~100 transactions)
- Testnet: Any amount > 0 (faucet tokens)

If wallets are empty, ask user to fund:
- Provide faucet links for testnet
- User must send mainnet tokens manually

### 2.3 Summary Before Implementation

Present a summary to user before proceeding:

```
Network: Scroll
Chain IDs: 534352 (mainnet), 534351 (testnet)
USDC Contracts:
  - Mainnet: 0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4
  - Testnet: 0x...
EIP-3009: Verified
EIP-712 Domain: name="USD Coin", version="2"

Prerequisites:
  - Logo: static/scroll.png EXISTS
  - Mainnet wallet: 0.05 ETH FUNDED
  - Testnet wallet: 0.1 ETH FUNDED

Ready to implement!
```

---

## Phase 3: Implementation

### 3.1 Update src/network.rs

**Add Network enum variants:**

```rust
/// Scroll mainnet (chain ID 534352).
#[serde(rename = "scroll")]
Scroll,
/// Scroll Sepolia testnet (chain ID 534351).
#[serde(rename = "scroll-sepolia")]
ScrollSepolia,
```

**Add to ALL `variants()` arrays** (there are 4):
1. `variants()` - all networks
2. `mainnet_variants()` - mainnet only
3. `testnet_variants()` - testnet only
4. `evm_variants()` - EVM networks

**Add Display impl:**

```rust
Self::Scroll => "Scroll",
Self::ScrollSepolia => "Scroll Sepolia",
```

**Add FromStr impl:**

```rust
"scroll" => Ok(Self::Scroll),
"scroll-sepolia" => Ok(Self::ScrollSepolia),
```

**Add to_caip2():**

```rust
Self::Scroll => "eip155:534352".to_string(),
Self::ScrollSepolia => "eip155:534351".to_string(),
```

**Add NetworkFamily mapping:**

```rust
Self::Scroll | Self::ScrollSepolia => NetworkFamily::Evm,
```

**Add USDC deployment constants:**

```rust
// ============================================================================
// USDC on Scroll
// ============================================================================

static USDC_SCROLL: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4").into(),
            network: Network::Scroll,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

static USDC_SCROLL_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("...TESTNET_ADDRESS...").into(),
            network: Network::ScrollSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});
```

**Add to usdc_deployments():**

```rust
Self::Scroll => Some(USDC_SCROLL.clone()),
Self::ScrollSepolia => Some(USDC_SCROLL_SEPOLIA.clone()),
```

### 3.2 Update src/from_env.rs

**Add RPC constants:**

```rust
pub const ENV_RPC_SCROLL: &str = "RPC_URL_SCROLL";
pub const ENV_RPC_SCROLL_SEPOLIA: &str = "RPC_URL_SCROLL_SEPOLIA";
```

**Add to rpc_env_name_from_network():**

```rust
Network::Scroll => ENV_RPC_SCROLL,
Network::ScrollSepolia => ENV_RPC_SCROLL_SEPOLIA,
```

### 3.3 Update src/chain/evm.rs

**Add chain ID mappings in TryFrom<Network>:**

```rust
Network::Scroll => Ok(EvmChain::new(value, 534352)),
Network::ScrollSepolia => Ok(EvmChain::new(value, 534351)),
```

**Add EIP-1559 support (usually true for modern chains):**

```rust
Network::Scroll => true,
Network::ScrollSepolia => true,
```

**IMPORTANT:** Some chains like SKALE don't support EIP-1559. Set to `false` for those.

### 3.4 Update src/chain/solana.rs

**Add UnsupportedNetwork exclusions:**

```rust
Network::Scroll => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
Network::ScrollSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
```

### 3.5 Update src/handlers.rs

**Add logo handler:**

```rust
pub async fn get_scroll_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/scroll.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}
```

**Add route:**

```rust
.route("/scroll.png", get(get_scroll_logo))
```

### 3.6 Update static/index.html

**Add CSS styling:**

```css
.network-badge.scroll {
    border-color: #FFEEDA;
    box-shadow: 0 0 20px rgba(255, 238, 218, 0.2);
}

.network-badge.scroll:hover {
    border-color: #FFEEDA;
    box-shadow: 0 8px 24px rgba(255, 238, 218, 0.3);
}
```

**Add mainnet card:**

```html
<div class="network-badge scroll"
    style="padding: 1.25rem 2rem; font-size: 1.1rem; flex-direction: column; gap: 0.75rem; min-height: 220px; width: 100%; cursor: pointer;"
    onclick="window.open('https://scrollscan.com/address/0x103040545AC5031A11E8C03dd11324C7333a13C7', '_blank')">
    <div style="display: flex; align-items: center; gap: 0.75rem;">
        <img src="/scroll.png" alt="Scroll" style="width: 32px; height: 32px; object-fit: contain;">
        <span style="font-weight: 700;">Scroll</span>
    </div>
    <div class="balance-amount" data-balance="scroll-mainnet"
        style="font-size: 1.1rem; font-weight: 700; font-family: 'JetBrains Mono', monospace; color: rgba(255,255,255,0.9);">
        Loading...
    </div>
    <div style="font-size: 0.7rem; color: var(--text-muted);">ETH Balance</div>
    <div data-tokens="scroll-mainnet"></div>
</div>
```

**Add testnet card** (similar structure).

**Add BALANCE_CONFIG entries:**

```javascript
'scroll-mainnet': {
    rpc: 'https://rpc.scroll.io',
    address: MAINNET_ADDRESS
},
'scroll-testnet': {
    rpc: 'https://sepolia-rpc.scroll.io',
    address: TESTNET_ADDRESS
}
```

**Add TOKEN_SUPPORT entries:**

```javascript
'scroll-mainnet': ['usdc'],
'scroll-testnet': ['usdc']
```

### 3.7 Update .env.example

```bash
# Scroll RPCs
RPC_URL_SCROLL=https://rpc.scroll.io
RPC_URL_SCROLL_SEPOLIA=https://sepolia-rpc.scroll.io
```

### 3.8 Update README.md

- Update network counts (mainnets and testnets)
- Add to supported networks table
- Run `python scripts/stablecoin_matrix.py --md` and update stablecoin table

---

## Phase 4: Build and Verify Locally

```bash
# Build
cargo build --release

# Check for errors
cargo clippy --all-targets

# Run locally
cargo run --release

# Verify network appears
curl http://localhost:8080/supported | jq '[.kinds[].network] | map(select(contains("scroll")))'
```

---

## Phase 5: Deploy

### If prerequisites met and user approves auto-deploy:

Use `/ship` skill which handles:
1. Version bump
2. Commit
3. Docker build
4. ECR push
5. ECS deploy
6. Verification

### Manual deploy steps:

```bash
# 1. Version bump
# Edit Cargo.toml version

# 2. Commit
git add -A && git commit -m "feat: add {Network} mainnet and testnet support"

# 3. Build
cargo build --release

# 4. Docker build and push
./scripts/build-and-push.sh vX.Y.Z

# 5. Update task definition (if using premium RPC)
# Add to AWS Secrets Manager if needed

# 6. Deploy
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2

# 7. Verify
curl https://facilitator.ultravioletadao.xyz/version
curl https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[].network] | map(select(contains("scroll")))'
```

---

## Automatic Deployment Decision

**Deploy automatically when ALL conditions are met:**
- Logo exists in `static/`
- Mainnet wallet balance > 0.001 ETH equivalent
- Testnet wallet balance > 0
- EIP-3009 verified on USDC contracts
- User has not requested manual review

**Request user confirmation when:**
- Any prerequisite is missing
- Chain has unusual characteristics (no EIP-1559, special gas token)
- Premium RPC required (API key needed)
- This is the first time adding this type of chain

---

## Troubleshooting

### "Network not in /supported"
- Check RPC environment variable is set
- Check wallet is funded
- Check `variants()` arrays include new network

### "Logo 404"
- Verify file exists: `ls static/{network}.png`
- Verify handler added to handlers.rs
- Verify route added to router
- Rebuild Docker image

### "Balance shows Loading..."
- Check BALANCE_CONFIG has correct RPC URL
- Check data-balance attribute matches config key
- Test RPC endpoint manually

### "Invalid signature" on payments
- EIP-712 name doesn't match (check contract!)
- EIP-712 version wrong
- Chain uses different signature format

---

## File Changes Summary

| File | Changes |
|------|---------|
| `src/network.rs` | Enum, Display, FromStr, CAIP-2, NetworkFamily, USDC deployments (~80 lines) |
| `src/from_env.rs` | RPC constants, match arms (~6 lines) |
| `src/chain/evm.rs` | Chain IDs, EIP-1559 flags (~4 lines) |
| `src/chain/solana.rs` | Exclusions (~2 lines) |
| `src/handlers.rs` | Logo handler, route (~12 lines) |
| `static/index.html` | CSS, cards, balance config, TOKEN_SUPPORT (~60 lines) |
| `static/{network}.png` | Logo file (1 file) |
| `.env.example` | RPC URLs (~2 lines) |
| `README.md` | Network counts, tables (~10 lines) |

**Total: ~175 lines + 1 logo file**

---

## Examples

### Example 1: Scroll (all prerequisites met)

```
User: add facilitator scroll
Claude: [Research phase - finds all data]
Claude: [Checks prerequisites]
  - Logo: static/scroll.png EXISTS
  - Mainnet: 0.05 ETH FUNDED
  - Testnet: 0.1 ETH FUNDED
Claude: All prerequisites met! Implementing and deploying...
[Implements all changes]
[Calls /ship skill]
Claude: Scroll deployed! Verified at https://facilitator.ultravioletadao.xyz/supported
```

### Example 2: New chain (missing logo)

```
User: add facilitator linea
Claude: [Research phase]
Claude: [Checks prerequisites]
  - Logo: MISSING - Please provide static/linea.png
  - Mainnet: 0.02 ETH FUNDED
  - Testnet: 0.05 ETH FUNDED
Claude: I need the Linea logo (PNG, transparent background). Please place it at static/linea.png and let me know when ready.
User: [provides logo]
Claude: Logo found! Continuing with implementation and deploy...
```

### Example 3: Wallets not funded

```
User: add facilitator blast
Claude: [Research phase]
Claude: [Checks prerequisites]
  - Logo: static/blast.png EXISTS
  - Mainnet: 0 ETH NOT FUNDED
  - Testnet: 0 ETH NOT FUNDED
Claude: Wallets need funding:
  - Mainnet (0x103040545AC5031A11E8C03dd11324C7333a13C7): Send ETH
  - Testnet (0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8): Use faucet at https://blastfaucet.com
Let me know when funded!
```
