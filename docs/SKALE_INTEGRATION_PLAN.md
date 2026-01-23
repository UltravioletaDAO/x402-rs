# SKALE Network Integration Plan

**Date**: 2026-01-22
**Target Version**: v1.20.0
**Networks**: SKALE Base Mainnet + SKALE Base Sepolia (Testnet)

---

## Executive Summary

Integrating SKALE Base (L3 sobre Base) al facilitador x402-rs. SKALE usa el modelo gasless (sFUEL es gratis), lo que significa que el facilitador no pagara gas real por settlements.

### Network Information

| Property | Mainnet | Testnet |
|----------|---------|---------|
| **Name** | SKALE Base | SKALE Base Sepolia |
| **Chain ID** | `1187947933` | `324705682` |
| **RPC URL** | `https://skale-base.skalenodes.com/v1/base` | `https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha` |
| **Explorer** | `https://skale-base.explorer.skalenodes.com/` | `https://base-sepolia-testnet-explorer.skalenodes.com/` |
| **Native Token** | sFUEL (free) | sFUEL (free) |
| **EIP-1559** | No (use --legacy) | No (use --legacy) |

### Stablecoin Deployments

| Token | Mainnet Address | Testnet Address | EIP-3009 |
|-------|-----------------|-----------------|----------|
| **USDC.e** | `0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20` | `0x2e08028E3C4c2356572E096d8EF835cD5C6030bD` | Native |

### EIP-712 Domain (USDC)

```json
{
  "name": "USDC",
  "version": "2",
  "chainId": 1187947933,
  "verifyingContract": "0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20"
}
```

---

## Task Division

### CLAUDE Tasks (Code Changes)

| # | Task | File(s) | Status |
|---|------|---------|--------|
| 1 | Add Network enum variants | `src/network.rs` | Done |
| 2 | Add USDC.e token deployments | `src/network.rs` | Done |
| 3 | Add Display impl for networks | `src/network.rs` | Done |
| 4 | Add FromStr impl for networks | `src/network.rs` | Done |
| 5 | Add to Network::variants() | `src/network.rs` | Done |
| 6 | Add RPC environment constants | `src/from_env.rs` | Done |
| 7 | Add chain ID mappings | `src/chain/evm.rs` | Done |
| 8 | Add EIP-1559 = false | `src/chain/evm.rs` | Done |
| 9 | Add Solana exclusions | `src/chain/solana.rs` | Done |
| 10 | Add logo handler | `src/handlers.rs` | Done |
| 11 | Add network cards to HTML | `static/index.html` | Done |
| 12 | Add CSS styling | `static/index.html` | Done |
| 13 | Add balance config JS | `static/index.html` | Done |
| 14 | Update .env.example | `.env.example` | Done |
| 15 | Update README network count | `README.md` | Done |

### USER Tasks (Infrastructure)

| # | Task | Details | Status |
|---|------|---------|--------|
| A | Provide SKALE logo | PNG, 32x32 or 64x64, transparent background | Pending |
| B | Get sFUEL for mainnet wallet | Faucet: https://www.sfuelstation.com/ | Pending |
| C | Get sFUEL for testnet wallet | Faucet: https://sfuel.dirtroad.dev/staging | Pending |
| D | Update AWS Secrets Manager | Add SKALE RPC if using premium endpoint | Optional |
| E | Update ECS task definition | Add RPC_URL_SKALE_BASE and RPC_URL_SKALE_BASE_SEPOLIA | Pending |
| F | Build and push Docker image | `./scripts/build-and-push.sh v1.20.0` | Pending |
| G | Deploy to ECS | `aws ecs update-service --force-new-deployment` | Pending |

---

## Detailed Steps

### Phase 1: Code Changes (CLAUDE)

#### Step 1.1: Add Network Enum Variants

**File**: `src/network.rs`

```rust
/// SKALE Base mainnet (chain ID 1187947933).
#[serde(rename = "skale-base")]
SkaleBase,
/// SKALE Base Sepolia testnet (chain ID 324705682).
#[serde(rename = "skale-base-sepolia")]
SkaleBaseSepolia,
```

#### Step 1.2: Add USDC Deployments

**File**: `src/network.rs`

```rust
// SKALE Base mainnet USDC.e
static USDC_SKALE_BASE: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20").into(),
            network: Network::SkaleBase,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

// SKALE Base Sepolia testnet USDC.e
static USDC_SKALE_BASE_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x2e08028E3C4c2356572E096d8EF835cD5C6030bD").into(),
            network: Network::SkaleBaseSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});
```

#### Step 1.3: Add Chain ID Mappings

**File**: `src/chain/evm.rs`

```rust
Network::SkaleBase => Ok(EvmChain::new(value, 1187947933)),
Network::SkaleBaseSepolia => Ok(EvmChain::new(value, 324705682)),
```

#### Step 1.4: Add EIP-1559 = false

**File**: `src/chain/evm.rs`

SKALE does NOT support EIP-1559, must use legacy transactions:

```rust
Network::SkaleBase => false,
Network::SkaleBaseSepolia => false,
```

#### Step 1.5: Add Logo Handler

**File**: `src/handlers.rs`

```rust
pub async fn get_skale_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/skale.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

// In facilitator_router():
.route("/skale.png", get(get_skale_logo))
```

#### Step 1.6: Add Network Cards

**File**: `static/index.html`

Mainnet card (in mainnet section):
```html
<div class="network-badge skale" style="padding: 1.25rem 2rem; font-size: 1.1rem; flex-direction: column; gap: 0.75rem; min-height: 180px; width: 100%; cursor: pointer;" onclick="window.open('https://skale-base.explorer.skalenodes.com/address/MAINNET_WALLET', '_blank')">
    <div style="display: flex; align-items: center; gap: 0.75rem;">
        <img src="/skale.png" alt="SKALE" style="width: 32px; height: 32px; object-fit: contain;">
        <span style="font-weight: 700;">SKALE Base</span>
    </div>
    <div class="balance-amount" data-balance="skale-base-mainnet" style="font-size: 1.1rem; font-weight: 700; font-family: 'JetBrains Mono', monospace; color: rgba(255,255,255,0.9);">
        Loading...
    </div>
    <div style="font-size: 0.7rem; color: var(--text-muted);">sFUEL Balance</div>
</div>
```

#### Step 1.7: Add CSS Styling

**File**: `static/index.html` (style section)

```css
.network-badge.skale {
    border-color: #00d4aa;
    box-shadow: 0 0 20px rgba(0, 212, 170, 0.2);
}

.network-badge.skale:hover {
    border-color: #00d4aa;
    box-shadow: 0 8px 24px rgba(0, 212, 170, 0.3);
}
```

#### Step 1.8: Add Balance Config

**File**: `static/index.html` (JavaScript section)

```javascript
'skale-base-mainnet': {
    rpc: 'https://skale-base.skalenodes.com/v1/base',
    address: MAINNET_ADDRESS
},
'skale-base-testnet': {
    rpc: 'https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha',
    address: TESTNET_ADDRESS
}
```

---

### Phase 2: User Tasks

#### Step 2.A: Provide SKALE Logo

1. Download official SKALE logo from https://skale.space/brand
2. Convert to PNG with transparent background
3. Resize to 32x32 or 64x64 pixels
4. Save as `static/skale.png`

#### Step 2.B: Get sFUEL for Mainnet Wallet

1. Go to https://www.sfuelstation.com/
2. Select "SKALE on Base" network
3. Enter mainnet facilitator wallet address: `0x103040545AC5031A11E8C03dd11324C7333a13C7`
4. Request sFUEL (it's free)
5. Verify balance on explorer: https://skale-base.explorer.skalenodes.com/address/0x103040545AC5031A11E8C03dd11324C7333a13C7

#### Step 2.C: Get sFUEL for Testnet Wallet

1. Go to https://sfuel.dirtroad.dev/staging
2. Select SKALE Base Sepolia network
3. Enter testnet facilitator wallet address: `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`
4. Request sFUEL
5. Verify balance on testnet explorer: https://base-sepolia-testnet-explorer.skalenodes.com/address/0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8

#### Step 2.D: Update AWS Secrets Manager (Optional)

If using premium RPC instead of public endpoints:

```bash
# Get current secret
aws secretsmanager get-secret-value \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --query SecretString \
  --output text > current_secret.json

# Edit to add: "skale-base": "https://your-premium-rpc-url"

# Update secret
aws secretsmanager update-secret \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --secret-string file://current_secret.json
```

#### Step 2.E: Update ECS Task Definition

Add to `environment` array (public RPCs are fine for SKALE):

```json
{
  "name": "RPC_URL_SKALE_BASE",
  "value": "https://skale-base.skalenodes.com/v1/base"
},
{
  "name": "RPC_URL_SKALE_BASE_SEPOLIA",
  "value": "https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha"
}
```

#### Step 2.F: Build and Deploy

```bash
# Bump version in Cargo.toml to v1.20.0
# Then build and push
./scripts/build-and-push.sh v1.20.0

# Or manually:
docker build --platform linux/amd64 \
  --build-arg FACILITATOR_VERSION=v1.20.0 \
  -t facilitator:v1.20.0 .

docker tag facilitator:v1.20.0 \
  518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.20.0

aws ecr get-login-password --region us-east-2 | docker login --username AWS --password-stdin 518898403364.dkr.ecr.us-east-2.amazonaws.com

docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.20.0
```

#### Step 2.G: Deploy to ECS

```bash
# Register new task definition
aws ecs register-task-definition \
  --cli-input-json file://terraform/environments/production/task-def-final.json \
  --region us-east-2

# Get new revision
REVISION=$(aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 \
  --query 'taskDefinition.revision' \
  --output text)

# Update service
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:$REVISION \
  --force-new-deployment \
  --region us-east-2
```

---

### Phase 3: Verification

#### Verify Version

```bash
curl https://facilitator.ultravioletadao.xyz/version
# Expected: {"version":"v1.20.0"}
```

#### Verify Networks in /supported

```bash
curl https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | contains("skale"))'
```

Expected:
```json
{
  "network": "skale-base",
  "scheme": "exact",
  "x402Version": 1
}
{
  "network": "skale-base-sepolia",
  "scheme": "exact",
  "x402Version": 1
}
```

#### Verify Logo

```bash
curl -I https://facilitator.ultravioletadao.xyz/skale.png
# Expected: HTTP/2 200, content-type: image/png
```

#### Verify Frontend

1. Open https://facilitator.ultravioletadao.xyz
2. Check SKALE cards appear in mainnet and testnet sections
3. Verify logo displays correctly
4. Verify teal/green border styling
5. Verify balance loads (should show sFUEL)
6. Click card - should open SKALE explorer

---

## Checklist Summary

### Before Starting
- [ ] SKALE logo file ready (PNG, transparent)
- [ ] sFUEL obtained for mainnet wallet
- [ ] sFUEL obtained for testnet wallet

### Code Changes (Claude)
- [ ] Network enum variants added
- [ ] USDC deployments added
- [ ] Display/FromStr impls updated
- [ ] RPC constants added
- [ ] Chain ID mappings added
- [ ] EIP-1559 = false set
- [ ] Solana exclusions added
- [ ] Logo handler added
- [ ] HTML cards added
- [ ] CSS styling added
- [ ] Balance config added
- [ ] .env.example updated

### Deployment (User)
- [ ] Logo file placed in static/
- [ ] Cargo.toml version bumped
- [ ] Docker image built
- [ ] Docker image pushed to ECR
- [ ] Task definition updated
- [ ] Task definition registered
- [ ] ECS service updated
- [ ] Deployment verified

### Post-Deployment
- [ ] /version returns correct version
- [ ] /supported shows skale-base and skale-base-sepolia
- [ ] Logo accessible at /skale.png
- [ ] Frontend displays cards correctly
- [ ] Balances load successfully

---

## Important Notes

### sFUEL is Free!

SKALE's gas model is unique:
- **sFUEL has no monetary value** - it's free compute credits
- Facilitator wallet does NOT need ETH/USDC for gas
- Just request sFUEL from faucet and you're set

### EIP-1559 Not Supported

SKALE uses legacy transactions. The code already handles this via `is_eip1559()` returning `false`.

### Brand Color

SKALE brand color: `#00d4aa` (teal/mint green)

### Network Naming

- Mainnet: `skale-base` (matches PayAI's naming)
- Testnet: `skale-base-sepolia`

---

## Resources

- [SKALE Documentation](https://docs.skale.space/)
- [SKALE Base on ChainList](https://chainlist.org/chain/1187947933)
- [sFUEL Station (Mainnet)](https://www.sfuelstation.com/)
- [sFUEL Faucet (Testnet)](https://sfuel.dirtroad.dev/staging)
- [SKALE Brand Assets](https://skale.space/brand)
- [PayAI SKALE Integration](https://blog.skale.space/blog/payai-launches-on-skale)
