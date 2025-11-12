# Adding New Blockchain Networks to x402-rs Facilitator

This guide provides a complete checklist and step-by-step instructions for adding new blockchain networks to the facilitator. Based on the Unichain integration (v1.3.4-v1.3.5), this ensures all components are properly configured.

## Overview

Adding a new chain requires:
1. Backend integration (Rust code)
2. RPC endpoint configuration (AWS Secrets Manager + public RPCs)
3. Frontend integration (HTML/CSS/JavaScript)
4. Logo assets and handlers
5. Wallet funding for both mainnet and testnet
6. Docker build and deployment
7. Verification and testing

## Prerequisites Checklist

Before starting, gather the following information:

- [ ] **Network Information**
  - [ ] Network name (e.g., "Unichain")
  - [ ] Mainnet chain ID (e.g., 130)
  - [ ] Testnet chain ID (e.g., 1301)
  - [ ] Block explorer URLs (mainnet and testnet)
  - [ ] Network type (EVM or Solana)
  - [ ] EIP-1559 support (yes/no)

- [ ] **USDC Contract Addresses**
  - [ ] Mainnet USDC contract address
  - [ ] Testnet USDC contract address
  - [ ] Token decimals (usually 6 for USDC)
  - [ ] EIP-712 domain info (name and version)

- [ ] **RPC Endpoints**
  - [ ] Premium/private RPC for mainnet (with API key)
  - [ ] Public RPC for mainnet (for frontend balance loading)
  - [ ] Public RPC for testnet (backend and frontend)

- [ ] **Assets**
  - [ ] Network logo (PNG, transparent background, ~32x32px recommended)
  - [ ] Brand color (hex code for CSS border styling)

- [ ] **Wallet Funding**
  - [ ] Mainnet facilitator wallet funded with native tokens (for gas)
  - [ ] Testnet facilitator wallet funded with native tokens (for gas)

## Step-by-Step Implementation

### Phase 1: Backend Integration (Rust)

#### 1.1 Update Network Enum

**File**: `src/network.rs`

Add new variants to the `Network` enum:

```rust
/// Unichain mainnet (chain ID 130).
#[serde(rename = "unichain")]
Unichain,
/// Unichain Sepolia testnet (chain ID 1301).
#[serde(rename = "unichain-sepolia")]
UnichainSepolia,
```

**Checklist**:
- [ ] Add mainnet variant with doc comment
- [ ] Add testnet variant with doc comment
- [ ] Use kebab-case for serde rename (e.g., "unichain-sepolia")
- [ ] Add to `Network::variants()` array
- [ ] Add to `Display` impl for human-readable names
- [ ] Add to `NetworkFamily` mapping (Evm or Solana)

#### 1.2 Add USDC Token Deployments

**File**: `src/network.rs`

Add `Lazy` static constants for USDC contracts:

```rust
static USDC_UNICHAIN: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x078D782b760474a361dDA0AF3839290b0EF57AD6").into(),
            network: Network::Unichain,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});
```

**Checklist**:
- [ ] Add mainnet USDC deployment
- [ ] Add testnet USDC deployment
- [ ] Verify contract addresses on block explorer
- [ ] Set correct decimals (usually 6)
- [ ] Add EIP-712 domain info (name and version)
- [ ] Add to `Network::usdc_deployments()` match statement

#### 1.3 Add RPC Environment Constants

**File**: `src/from_env.rs`

Add constants for RPC environment variables:

```rust
pub const ENV_RPC_UNICHAIN: &str = "RPC_URL_UNICHAIN";
pub const ENV_RPC_UNICHAIN_SEPOLIA: &str = "RPC_URL_UNICHAIN_SEPOLIA";
```

Add to `rpc_env_name_from_network()` match:

```rust
Network::Unichain => ENV_RPC_UNICHAIN,
Network::UnichainSepolia => ENV_RPC_UNICHAIN_SEPOLIA,
```

**Checklist**:
- [ ] Add mainnet RPC constant
- [ ] Add testnet RPC constant
- [ ] Add to `rpc_env_name_from_network()` match
- [ ] Follow naming convention: `RPC_URL_{NETWORK}_{ENVIRONMENT}`

#### 1.4 Add Chain ID Mappings

**File**: `src/chain/evm.rs` (for EVM chains)

Add chain ID mappings in `TryFrom<Network> for EvmChain`:

```rust
Network::Unichain => Ok(EvmChain::new(value, 130)),
Network::UnichainSepolia => Ok(EvmChain::new(value, 1301)),
```

Add EIP-1559 support in the `is_eip1559` match:

```rust
Network::Unichain => true,
Network::UnichainSepolia => true,
```

**Checklist**:
- [ ] Add mainnet chain ID mapping
- [ ] Add testnet chain ID mapping
- [ ] Set EIP-1559 support flag (true for modern chains)
- [ ] Verify chain IDs match network documentation

**For Solana chains**: Update `src/chain/solana.rs` instead

#### 1.5 Update Non-EVM Chain Exclusions

**File**: `src/chain/solana.rs` (if adding EVM chain)

Add exclusions for non-Solana networks:

```rust
Network::Unichain => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
Network::UnichainSepolia => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
```

**Checklist**:
- [ ] Add mainnet exclusion
- [ ] Add testnet exclusion

#### 1.6 Compile and Test Backend

```bash
cargo check --features solana
cargo build --release --features solana
```

**Checklist**:
- [ ] No compilation errors
- [ ] No non-exhaustive pattern warnings
- [ ] All match statements handle new networks

### Phase 2: RPC Configuration

#### 2.1 Add to Environment Example

**File**: `.env.example`

Add public RPC endpoints:

```bash
# Unichain RPCs
RPC_URL_UNICHAIN=https://unichain-rpc.publicnode.com
RPC_URL_UNICHAIN_SEPOLIA=https://unichain-sepolia.drpc.org
```

**Checklist**:
- [ ] Add mainnet public RPC (for local testing)
- [ ] Add testnet public RPC
- [ ] Document in comments if needed

#### 2.2 Configure AWS Secrets Manager (Mainnet Premium RPC)

**CRITICAL**: Never put RPC URLs with API keys in task definitions!

Update AWS Secrets Manager secret:

```bash
# Get current secret value
aws secretsmanager get-secret-value \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --query SecretString \
  --output text > current_secret.json

# Edit current_secret.json to add new network
# Add: "unichain": "https://node-name.unichain-mainnet.quiknode.pro/API_KEY/"

# Update secret
aws secretsmanager update-secret \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --secret-string file://current_secret.json

# Clean up
rm current_secret.json
```

**Checklist**:
- [ ] Get current mainnet RPC secret
- [ ] Add new network's premium RPC with API key
- [ ] Update AWS Secrets Manager secret
- [ ] Verify secret update succeeded
- [ ] Test RPC endpoint works

#### 2.3 Update Task Definition (for production deployment)

**File**: `task-def-final.json` (or your current task definition)

Add environment variable for testnet (public RPC):

```json
{
  "name": "RPC_URL_UNICHAIN_SEPOLIA",
  "value": "https://unichain-sepolia.drpc.org"
}
```

Add secret reference for mainnet (premium RPC):

```json
{
  "name": "RPC_URL_UNICHAIN",
  "valueFrom": "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-rpc-mainnet-5QJ8PN:unichain::"
}
```

**Checklist**:
- [ ] Add testnet RPC to `environment` array (public RPC OK)
- [ ] Add mainnet RPC to `secrets` array (references AWS Secrets Manager)
- [ ] Never put API keys directly in environment variables
- [ ] Follow ARN format exactly

### Phase 3: Frontend Integration

#### 3.1 Add Network Logo

**File**: `static/unichain.png`

**Checklist**:
- [ ] Logo is PNG format with transparent background
- [ ] Recommended size: 32x32px or 64x64px
- [ ] File named in lowercase (e.g., `unichain.png`)
- [ ] Place in `static/` directory
- [ ] Commit logo to git

#### 3.2 Add Logo Handler

**File**: `src/handlers.rs`

Add handler function:

```rust
pub async fn get_unichain_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/unichain.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}
```

Add route in `facilitator_router()`:

```rust
.route("/unichain.png", get(get_unichain_logo))
```

**Checklist**:
- [ ] Create handler function with `include_bytes!()` macro
- [ ] Return PNG content-type
- [ ] Add route to router
- [ ] Test handler compiles

#### 3.3 Add Network Cards to Landing Page

**File**: `static/index.html`

Add mainnet card in mainnet section:

```html
<div class="network-badge unichain" style="padding: 1.25rem 2rem; font-size: 1.1rem; flex-direction: column; gap: 0.75rem; min-height: 180px; width: 100%; cursor: pointer;" onclick="window.open('https://uniscan.xyz/address/0x103040545AC5031A11E8C03dd11324C7333a13C7', '_blank')">
    <div style="display: flex; align-items: center; gap: 0.75rem;">
        <img src="/unichain.png" alt="Unichain" style="width: 32px; height: 32px; object-fit: contain;">
        <span style="font-weight: 700;">Unichain</span>
    </div>
    <div class="balance-amount" data-balance="unichain-mainnet" style="font-size: 1.1rem; font-weight: 700; font-family: 'JetBrains Mono', monospace; color: rgba(255,255,255,0.9);">
        Loading...
    </div>
    <div style="font-size: 0.7rem; color: var(--text-muted);">ETH Balance</div>
</div>
```

Add testnet card in testnet section (similar structure).

**Checklist**:
- [ ] Add mainnet card with correct explorer link
- [ ] Add testnet card with correct explorer link
- [ ] Update facilitator wallet addresses in onclick attributes
- [ ] Use correct data-balance attribute for JavaScript
- [ ] Use correct native token name (ETH, AVAX, SOL, etc.)

#### 3.4 Add CSS Border Styling

**File**: `static/index.html` (in `<style>` section)

Add network-specific CSS:

```css
.network-badge.unichain {
    border-color: #ff1f8f;
    box-shadow: 0 0 20px rgba(255, 31, 143, 0.2);
}

.network-badge.unichain:hover {
    border-color: #ff1f8f;
    box-shadow: 0 8px 24px rgba(255, 31, 143, 0.3);
}
```

**Checklist**:
- [ ] Choose brand color (hex code)
- [ ] Add border-color and box-shadow
- [ ] Add hover effect with stronger shadow
- [ ] Convert hex to rgba for box-shadow

#### 3.5 Add Balance Loading Configuration

**File**: `static/index.html` (in JavaScript section)

Add to `BALANCE_CONFIG` object:

```javascript
'unichain-mainnet': {
    rpc: 'https://unichain-rpc.publicnode.com',
    address: MAINNET_ADDRESS
},
'unichain-testnet': {
    rpc: 'https://unichain-sepolia.drpc.org',
    address: TESTNET_ADDRESS
}
```

**Checklist**:
- [ ] Add mainnet balance config with public RPC
- [ ] Add testnet balance config with public RPC
- [ ] Use correct wallet address constants (MAINNET_ADDRESS or TESTNET_ADDRESS)
- [ ] Match data-balance attributes from HTML cards

### Phase 4: Wallet Funding

#### 4.1 Fund Mainnet Wallet

**CRITICAL**: Facilitator wallet needs **native tokens** (ETH, AVAX, SOL) for gas, not payment tokens!

**Checklist**:
- [ ] Identify facilitator mainnet wallet address
- [ ] Send native tokens to wallet (enough for ~100-1000 transactions)
- [ ] Verify balance on block explorer
- [ ] Test gas estimation for typical transaction

#### 4.2 Fund Testnet Wallet

**Checklist**:
- [ ] Identify facilitator testnet wallet address
- [ ] Get testnet tokens from faucet or bridge
- [ ] Verify balance on block explorer
- [ ] Test transaction on testnet

#### 4.3 Verify Wallet Separation

Ensure mainnet and testnet use separate wallets (v1.3.0+):

```bash
# Check environment variables
echo "Mainnet: $EVM_PRIVATE_KEY_MAINNET"
echo "Testnet: $EVM_PRIVATE_KEY_TESTNET"

# Or check AWS Secrets Manager
aws secretsmanager get-secret-value \
  --secret-id facilitator-evm-private-key-mainnet \
  --region us-east-2 | jq -r .SecretString

aws secretsmanager get-secret-value \
  --secret-id facilitator-evm-private-key-testnet \
  --region us-east-2 | jq -r .SecretString
```

**Checklist**:
- [ ] Mainnet and testnet use different wallet addresses
- [ ] Both wallets configured in AWS Secrets Manager (production)
- [ ] Both wallets have sufficient native token balances

### Phase 5: Build and Deployment

#### 5.1 Commit Changes

```bash
git add src/network.rs src/from_env.rs src/chain/evm.rs src/chain/solana.rs
git add src/handlers.rs static/index.html static/unichain.png .env.example
git commit -m "feat: Add Unichain mainnet and Sepolia testnet support (v1.3.X)"
```

**Checklist**:
- [ ] All modified files staged
- [ ] Logo file committed
- [ ] Clear commit message with network name and version

#### 5.2 Build Docker Image

```bash
# Determine next version (e.g., v1.3.4)
VERSION="v1.3.4"

# Build with version tag
docker build --platform linux/amd64 \
  --build-arg FACILITATOR_VERSION=$VERSION \
  -t facilitator:$VERSION .
```

**Checklist**:
- [ ] Determine semantic version number
- [ ] Include --build-arg FACILITATOR_VERSION
- [ ] Build succeeds without errors
- [ ] Image size reasonable (~500MB-1GB)

#### 5.3 Push to ECR

```bash
# Tag for ECR
docker tag facilitator:$VERSION \
  518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:$VERSION

# Push to ECR
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:$VERSION
```

**Checklist**:
- [ ] ECR login successful (`aws ecr get-login-password`)
- [ ] Image tagged correctly
- [ ] Push succeeds
- [ ] Verify image in ECR console

#### 5.4 Register Task Definition

Update image version in task definition:

```json
{
  "image": "518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.3.4"
}
```

Register new task definition:

```bash
aws ecs register-task-definition \
  --cli-input-json file://task-def-final.json \
  --region us-east-2
```

**Checklist**:
- [ ] Task definition JSON updated with new version
- [ ] All environment variables present
- [ ] All secret references correct
- [ ] New revision number returned

#### 5.5 Deploy to ECS

```bash
# Get new revision number (e.g., 33)
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

**Checklist**:
- [ ] Service update initiated
- [ ] New task starting (PRIMARY deployment)
- [ ] Old task draining (ACTIVE deployment)
- [ ] Wait for deployment to complete (~2-5 minutes)

#### 5.6 Tag Release

```bash
git tag $VERSION -m "Release $VERSION: Add Network support"
git push origin main --tags
```

**Checklist**:
- [ ] Git tag created
- [ ] Tag pushed to remote
- [ ] Tag matches Docker image version

### Phase 6: Verification and Testing

#### 6.1 Verify Version

```bash
curl https://facilitator.ultravioletadao.xyz/version
# Expected: {"version":"v1.3.4"}
```

**Checklist**:
- [ ] Version endpoint returns correct version
- [ ] Matches deployed Docker image tag

#### 6.2 Verify Networks in /supported

```bash
curl https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | contains("unichain"))'
```

Expected output:

```json
{
  "network": "unichain",
  "scheme": "exact",
  "x402Version": 1
}
{
  "network": "unichain-sepolia",
  "scheme": "exact",
  "x402Version": 1
}
```

**Checklist**:
- [ ] Mainnet network appears in /supported
- [ ] Testnet network appears in /supported
- [ ] Total network count increased by 2

#### 6.3 Verify Logo Accessible

```bash
curl -I https://facilitator.ultravioletadao.xyz/unichain.png
# Expected: HTTP/2 200
```

**Checklist**:
- [ ] Logo returns HTTP 200
- [ ] Content-Type is image/png
- [ ] Logo displays correctly in browser

#### 6.4 Verify Frontend

Open https://facilitator.ultravioletadao.xyz in browser:

**Checklist**:
- [ ] Network cards visible in mainnet and testnet sections
- [ ] Logo displays correctly
- [ ] Border styling applied (colored border)
- [ ] Balance loading shows "Loading..." then actual balance
- [ ] Click on card opens correct block explorer
- [ ] Explorer shows facilitator wallet address

#### 6.5 Test Payment Flow (Optional but Recommended)

For comprehensive testing, test a payment on testnet:

```bash
cd tests/integration
python test_usdc_payment.py --network unichain-sepolia
```

**Checklist**:
- [ ] Verify endpoint accepts network parameter
- [ ] Payment verification succeeds
- [ ] Settlement succeeds (if testing settlement)
- [ ] Transaction appears on block explorer

### Phase 7: Documentation

#### 7.1 Update CLAUDE.md

Already done! See updated CLAUDE.md pointing to this guide.

**Checklist**:
- [ ] CLAUDE.md references this guide
- [ ] Network count updated in CLAUDE.md if documented

#### 7.2 Update CHANGELOG.md (if exists)

Add entry to CHANGELOG:

```markdown
## [v1.3.4] - 2025-11-12

### Added
- Unichain mainnet and Sepolia testnet support
- Network cards with pink border styling
- Logo handler for /unichain.png endpoint
- Premium RPC configuration in AWS Secrets Manager
```

**Checklist**:
- [ ] CHANGELOG entry added
- [ ] Version number matches release
- [ ] All major changes documented

## Common Issues and Troubleshooting

### Issue: Network not appearing in /supported

**Possible causes**:
- RPC environment variable not configured
- RPC endpoint not reachable
- Wallet not funded with gas

**Solution**:
1. Check CloudWatch logs for warnings: `no RPC URL configured, skipping`
2. Verify environment variables in task definition
3. Test RPC endpoint: `curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' $RPC_URL`
4. Check wallet balance on block explorer

### Issue: Logo returns 404

**Possible causes**:
- Logo file not committed to git
- Handler function not added
- Route not registered
- Docker image built before logo was committed

**Solution**:
1. Verify logo file exists: `ls -lh static/unichain.png`
2. Check git status: `git status static/unichain.png`
3. Verify handler in src/handlers.rs
4. Rebuild Docker image
5. Push new image to ECR
6. Force new deployment

### Issue: Border styling not showing

**Possible causes**:
- CSS not added for network class
- Class name mismatch between HTML and CSS
- Browser caching old CSS

**Solution**:
1. Verify CSS exists: `grep -A 4 "\.network-badge\.unichain" static/index.html`
2. Check class name in HTML matches CSS selector
3. Hard refresh browser (Ctrl+Shift+R)
4. Verify Docker image includes updated index.html

### Issue: Balance shows "Loading..." forever

**Possible causes**:
- Public RPC endpoint not responding
- JavaScript configuration mismatch
- CORS issues with RPC endpoint

**Solution**:
1. Test RPC endpoint in browser console
2. Verify balance config key matches data-balance attribute
3. Check browser console for JavaScript errors
4. Try different public RPC endpoint

### Issue: Payment verification fails

**Possible causes**:
- Wrong USDC contract address
- Wallet not funded with gas
- Invalid EIP-712 domain info
- RPC timeout or rate limiting

**Solution**:
1. Verify USDC contract address on block explorer
2. Check facilitator wallet balance (native tokens, not USDC!)
3. Verify EIP-712 domain matches USDC contract
4. Use premium RPC endpoint to avoid rate limits
5. Check CloudWatch logs for specific error messages

## Security Reminders

- **NEVER** commit private keys to git
- **NEVER** put RPC URLs with API keys in task definition environment variables
- **ALWAYS** use AWS Secrets Manager for RPC URLs with API keys
- **ALWAYS** use separate wallets for mainnet and testnet (v1.3.0+)
- **ALWAYS** rotate API keys if accidentally exposed

## Quick Reference: File Changes Summary

For adding a new chain named "NewChain" with mainnet and testnet:

| File | Changes | Count |
|------|---------|-------|
| `src/network.rs` | Add enum variants, USDC deployments, display names | ~80 lines |
| `src/from_env.rs` | Add RPC constants, update match | ~6 lines |
| `src/chain/evm.rs` | Add chain IDs, EIP-1559 flags | ~4 lines |
| `src/chain/solana.rs` | Add exclusions (if EVM) | ~2 lines |
| `src/handlers.rs` | Add logo handler and route | ~12 lines |
| `static/index.html` | Add cards, CSS, balance config | ~40 lines |
| `static/newchain.png` | Add logo file | 1 file |
| `.env.example` | Add RPC URLs | ~2 lines |
| `task-def-final.json` | Add env vars and secrets | ~8 lines |

**Total**: ~155 lines of code + 1 logo file + AWS Secrets Manager update + wallet funding

## Success Criteria

Your integration is complete when:

- [ ] Both mainnet and testnet networks appear in `/supported` endpoint
- [ ] Logo accessible at `/{network}.png` with HTTP 200
- [ ] Network cards display on landing page with correct styling
- [ ] Balances load correctly for both mainnet and testnet
- [ ] Clicking cards opens correct block explorer links
- [ ] No compilation errors or warnings
- [ ] Docker image builds successfully
- [ ] Deployment completes without errors
- [ ] Version endpoint returns correct version
- [ ] Both wallets funded with sufficient native tokens
- [ ] CloudWatch logs show no RPC errors for new networks

---

**Document Version**: 1.0 (2025-11-12)
**Based on**: Unichain integration (v1.3.4-v1.3.5)
**Last Updated**: 2025-11-12
