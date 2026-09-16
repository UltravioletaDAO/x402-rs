# Adding New Blockchain Networks to x402-rs Facilitator

This guide provides a complete checklist and step-by-step instructions for adding new blockchain networks to the facilitator. Based on the Unichain integration (v1.3.4-v1.3.5), this ensures all components are properly configured.

## Overview

Adding a new chain requires:
1. Backend integration (Rust code)
2. RPC endpoint configuration (Terraform task definition + AWS Secrets Manager)
3. Frontend integration (HTML/CSS/JavaScript)
4. Logo assets and handlers
5. Wallet funding for both mainnet and testnet
6. `VERSION` bump, per-file staging, push to `main` (CI deploys)
7. Verification against `/supported`

**17 files, always** — see [Quick Reference: File Changes Summary](#quick-reference-file-changes-summary).

**The alta is finished when the chain answers on `GET /supported`, and at no
earlier point.** A green `cargo build` proves nothing: three networks in the enum
compile, parse and serialize today and are served nowhere. See
[Success criteria](#success-criteria).

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
- [ ] Add to **all four copies** of `Network::variants()` — see below
- [ ] Add to `Display` impl for human-readable names
- [ ] Add to `FromStr` (`src/network.rs:216`)
- [ ] Add to `to_caip2()` (`src/network.rs:576`)
- [ ] Add to `from_caip2()` (`src/network.rs:643`) — a *separate* match
- [ ] Add to `NetworkFamily` mapping (Evm or Solana)

##### The four copies of `variants()`

There is one `Network::variants()`, written out four times, each behind a
different `algorand` / `sui` feature combination. There is no
`mainnet_variants()`, no `testnet_variants()` and no `evm_variants()` — those
three names appear nowhere in the source:

```bash
grep -rn -e mainnet_variants -e testnet_variants -e evm_variants src/ crates/ examples/
# 0 hits (measured 2026-09-16 on dc109511)
```

| `#[cfg(...)]` above it | line | entries |
|---|---|---|
| `all(feature = "algorand", feature = "sui")` | `src/network.rs:346` | 39 |
| `all(feature = "algorand", not(feature = "sui"))` | `src/network.rs:394` | 37 |
| `all(not(feature = "algorand"), feature = "sui")` | `src/network.rs:440` | 37 |
| `all(not(feature = "algorand"), not(feature = "sui"))` | `src/network.rs:486` | 35 |

An EVM chain goes in all four. Only the first is compiled by the production
feature set, so three of them can be wrong with a green build.

##### Which of these the compiler enforces

`Display`, `to_caip2()` and the `NetworkFamily` mapping are exhaustive matches:
skip one and the build breaks, naming your network. The other three are not:

| Site | Why it stays silent |
|---|---|
| the four `variants()` copies | an array, not a `match` |
| `FromStr` | `_ => Err(NetworkParseError(...))`, `src/network.rs:269` |
| `from_caip2()` | `_ => None`, `src/network.rs:704` |

Those three are where a network goes missing. See
[Success criteria](#success-criteria) at the end of this guide.

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

# The gate CI actually runs; a red run here blocks the production deploy.
# --test-threads=1 is not optional: parallel runs hang on CI runners.
cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1

# And build the OTHER three feature combinations, because each compiles a
# DIFFERENT copy of variants():
cargo check --features algorand
cargo check --features sui
cargo check
```

**Checklist**:
- [ ] No compilation errors
- [ ] No non-exhaustive pattern warnings
- [ ] All match statements handle the new network
- [ ] All four `variants()` copies updated — **the compiler cannot tell you this**;
      `variants()` is an array, not a `match`. A clean build here is not evidence.

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

#### 2.3 Give the container the RPC URL (Terraform)

**Files**: `terraform/environments/production/main.tf` (public RPC) and
`terraform/environments/production/secrets.tf` (RPC with an API key).

There is no hand-edited `task-def-final.json` in this repository — the task
definition is generated by Terraform. **This step is what decides whether the
network shows up in `/supported` at all.** `src/from_env.rs` only declares the
variable NAME; declared there alone, the container never receives a URL,
`NetworkProvider::from_env` returns `None`, and the chain is silently absent
behind a perfectly clean build.

Public / free RPC — `main.tf`, inside the task definition's `environment` block:

```hcl
{
  name  = "RPC_URL_UNICHAIN_SEPOLIA"
  value = "https://unichain-sepolia.drpc.org"
},
```

RPC URL carrying an API key — NEVER in `environment`. Put the key in the
`facilitator-rpc-mainnet` / `facilitator-rpc-testnet` secret (§2.2) and reference
it from the `secrets` block wired in `secrets.tf`. Task definitions are stored in
plaintext and their revision history is retained, so a key placed in
`environment` stays readable after it is rotated.

**Checklist**:
- [ ] Testnet RPC added to the `environment` block in `main.tf`
- [ ] Mainnet RPC added to `environment` (free endpoint) or to the secret + the
      `secrets` block (API key)
- [ ] Never put API keys directly in environment variables
- [ ] Both entries present — a missing one is invisible until `/supported`

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

### Phase 5: Ship it

**There is no manual deploy. Pushing to `main` IS the deploy.**

`.github/workflows/ci.yaml` runs the tests, builds the image, pushes it to ECR and
`terraform apply -auto-approve`s it onto ECS, then waits for the rollout and polls
`/health`. The `docker build` -> `docker push` -> `aws ecs
register-task-definition` -> `aws ecs update-service` sequence this phase used to
describe has not been how anything ships for a long time; there is no
`task-def-final.json` in the repository, and
`aws ecs update-service --force-new-deployment` on its own re-runs the CURRENT
task definition without moving the image.

#### 5.1 Bump `VERSION`

The release version lives in the `VERSION` file at the repo root, **not** in
`Cargo.toml` — `Cargo.toml:3` is a frozen `0.0.0` placeholder, and editing it
makes every deploy recompile the whole dependency tree (~12 min instead of ~6).
Bump from what is DEPLOYED, not from whatever is local:

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # e.g. {"version":"2.29.6"}
echo "2.30.0" > VERSION                                   # a network add is a minor bump
```

**Checklist**:
- [ ] `VERSION` bumped from the deployed version
- [ ] `Cargo.toml` untouched
- [ ] `docs/CHANGELOG.md` entry added

#### 5.2 Stage per file — never `git add -A`

`git add -A` is forbidden here. `.unused/` and untracked scratch files live beside
the tree, and the 2026-05-19 security audit recorded a wallet rotation script that
writes a freshly generated key into the repo root, where one `git add -A` would
publish it (`docs/reports/2026-05-19-security-audit.md`). Name every path, then
read back what you staged:

```bash
git add src/network.rs src/from_env.rs src/chain/evm.rs src/chain/solana.rs
git add src/handlers.rs src/openapi.rs
git add static/index.html static/x402.js static/unichain.png
git add config/supported_tokens.json lambda/balances/handler.py
git add terraform/environments/production/main.tf
git add scripts/verify_landing_canonical.py .env.example README.md
git add VERSION docs/CHANGELOG.md

git status --short          # anything unexpected staged is a stop
git diff --cached --stat
git commit -m "feat(network): add Unichain mainnet and Sepolia testnet (130/1301)"
```

Run `git config core.hooksPath .githooks` once per clone — the pre-commit hook
refuses a staged diff that adds `0x` followed by 64 hex characters.

**Checklist**:
- [ ] Every path named explicitly; no `git add -A`, no `git add .`
- [ ] `git status --short` and `git diff --cached --stat` both read before committing
- [ ] Logo file staged
- [ ] Pre-commit hook active

#### 5.3 Push and let CI ship it

```bash
git push origin main
```

What CI does, in order (`.github/workflows/ci.yaml`):

1. `test` — clippy plus
   `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1`,
   and the `--offline` landing-canonical guard. Red here blocks everything.
2. `preflight` — emits `deploy=true` when the AWS repo secrets are present.
3. drift gate — read-only `terraform plan`; it never applies.
4. `deploy` — the image tag is `$(cat VERSION)-$(git rev-parse --short HEAD)`,
   built with `--build-arg FACILITATOR_VERSION=$(cat VERSION)`, pushed to ECR,
   then a **targeted** `terraform apply` on the ECS task definition + service +
   autoscaling with `-var image_tag=...`. Never a full apply.
5. waits for the ECS rollout to stabilise, then polls `/health`.

Two things to know:

- The workflow has a `paths` filter. `docs/**`, `guides/**` and `.claude/**` are
  not in it, so a documentation-only commit never triggers CI and never deploys.
  Every other file in this guide's inventory is in it.
- A failed deploy leaves `main` ahead of production. Compare
  `curl -s https://facilitator.ultravioletadao.xyz/version` with `git log -1`
  before assuming your commit is live.

**Checklist**:
- [ ] `test` job green
- [ ] `deploy` job ran (not skipped) and finished
- [ ] `/version` matches the `VERSION` you pushed

#### 5.4 Do NOT tag the release

Releases stopped being git-tagged at `v2.0.2`, while `VERSION` is well past it.
`git tag` is not part of shipping; adding one now only creates a tag that
disagrees with every release since.

### Phase 6: Verification and Testing

#### 6.1 Verify Version

```bash
curl -s https://facilitator.ultravioletadao.xyz/version
# Expected: {"version":"2.30.0"} -- no leading "v"; it is the VERSION file verbatim
```

**Checklist**:
- [ ] `/version` matches the `VERSION` file you pushed
- [ ] If it still shows the previous version, the deploy did not land — check the
      CI run before debugging anything else

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

`tests/integration/test_usdc_payment.py` takes **no arguments**: it has no
`argparse`, and both the facilitator URL (`:19`) and the network (`:37`, `:114`,
`:122` — Base mainnet, chain 8453) are hardcoded. Running it does not test your
chain; it spends real money on Base.

To exercise the new network, post a `/verify` at it directly:

```bash
curl -s -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H 'Content-Type: application/json' \
  -d @payload.json | jq
```

**Checklist**:
- [ ] `/verify` reaches the new network's RPC (an error naming the chain is
      progress; "unsupported network" is not)
- [ ] Settlement succeeds, if testing settlement
- [ ] Transaction appears on the block explorer

### Phase 7: Documentation

#### 7.1 Update CLAUDE.md

Already done! See updated CLAUDE.md pointing to this guide.

**Checklist**:
- [ ] CLAUDE.md references this guide
- [ ] Network count updated in CLAUDE.md if documented

#### 7.2 Update `docs/CHANGELOG.md`

Not optional and not "if exists": `docs/CHANGELOG.md` is the only written record
of a release, because releases stopped being git-tagged at `v2.0.2`. A chain added
without a CHANGELOG entry has no date attached to it anywhere.

Add the entry:

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

#### 7.3 Create Integration Document

**REQUIRED**: Create `docs/{NETWORK}_INTEGRATION.md` with:

```markdown
# {Network Name} Network Integration

**Date:** {date}
**Version:** {version}
**Status:** Complete

## Network Details
- Chain ID, RPC URLs, Explorer
- CAIP-2 identifier

## Token Contracts
- USDC address
- EIP-3009 verification status
- EIP-712 domain info (name, version)

## Facilitator Wallet
- Mainnet/Testnet addresses
- Funding transactions (with etherscan links)

## Files Modified
- List all changed files with summary of changes

## Implementation Notes
- Any special considerations (EIP-1559, L3, etc.)

## Deployment Checklist
- All items from this guide
```

See examples:
- `docs/SKALE_INTEGRATION_PLAN.md`
- `docs/SCROLL_INTEGRATION.md`

**Checklist**:
- [ ] Integration document created
- [ ] Funding transaction linked
- [ ] All technical details documented

## Common Issues and Troubleshooting

### Issue: Network not appearing in /supported

In order of how often it is the actual cause:

1. **The container never received the RPC URL.** Declaring `ENV_RPC_*` in
   `src/from_env.rs` does nothing on its own — the value comes from the ECS task
   definition generated by `terraform/environments/production/main.tf` (or
   `secrets.tf` for a URL with an API key). See §2.3.
2. **The network is missing from one or more of the four `variants()` copies**
   (`src/network.rs:346`, `:394`, `:440`, `:486`). Nothing warns about this. Check
   all four, not the first one.
3. RPC endpoint unreachable from the task, or it rejects the facilitator's calls.
4. **The deploy never landed.** Compare
   `curl -s https://facilitator.ultravioletadao.xyz/version` with `git log -1`
   before debugging the code.

**Checks**:
1. CloudWatch: `no RPC URL configured, skipping`
2. The `environment` / `secrets` blocks in `terraform/environments/production/`
3. `curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' $RPC_URL`
4. Wallet balance on the block explorer

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

Measured, not estimated. Derived from `7dbe194e` — Robinhood Chain, 2026-07-20,
the last full network alta: **24 files, 689 insertions, 216 deletions**
(`git show --stat 7dbe194e`).

Five of those 24 are excluded below because they were not alta work:
`src/caip2.rs`, `src/chain/xrpl.rs`, `src/facilitator_local.rs` and
`examples/x402-reqwest-example/src/main.rs` were whole-file `rustfmt` with zero
mentions of the new chain, and `src/upto/permit2.rs` was an unrelated security fix
riding along. Two files have joined since: `VERSION` (did not exist at `7dbe194e`)
and `static/x402.js`.

### Always — 17 files

| # | File | Changes | Compiler catches an omission? |
|---|------|---------|---|
| 1 | `src/network.rs` | enum + serde rename, `Display`, `FromStr`, `to_caip2`, `from_caip2`, `NetworkFamily`, **4x `variants()`**, token deployments, `usdc_deployments()` (~226 lines measured) | partly — `variants()` x4, `FromStr`, `from_caip2` are NOT enforced |
| 2 | `src/from_env.rs` | `ENV_RPC_*` consts + `rpc_env_name_from_network()` | yes |
| 3 | `src/chain/evm.rs` | chain id in `TryFrom<Network>`, EIP-1559 flag; an `eip1559_fee_floor` arm only if the default is wrong for the chain | chain id yes; fee floor NO (`_` arm at `src/chain/evm.rs:329`) |
| 4 | `src/chain/solana.rs` | `UnsupportedNetwork` exclusion arms | yes |
| 5 | `src/handlers.rs` | logo handler + route | no |
| 6 | `src/openapi.rs` | network prose and lists — find them with `grep -n 'Scroll\|scroll\|mainnets\|networks (' src/openapi.rs` | no |
| 7 | `static/newchain.png` | logo, flat in `static/` (there is no `static/images/`) | build fails — `include_bytes!` |
| 8 | `static/index.html` | 2 balance cards + CSS + balance config | no |
| 9 | `static/x402.js` | `ICONO_DE_RED` at `:14` — 4 keys: v1 mainnet, v1 testnet, both CAIP-2 ids | no — silently falls back to a monogram |
| 10 | `config/supported_tokens.json` | chainId, tokens, explorer, facilitatorWallet | no |
| 11 | `lambda/balances/handler.py` | `get_network_configs()`: RPC + wallet | no |
| 12 | `terraform/environments/production/main.tf` | `RPC_URL_*` in the task definition | no — **and this is what keeps it out of `/supported`** |
| 13 | `scripts/verify_landing_canonical.py` | `--expect-mainnets` default (`:306`) and the docstring count (`:11`) | the guard itself, in CI |
| 14 | `.env.example` | both RPC URLs | no |
| 15 | `README.md` | network counts and tables | no |
| 16 | `VERSION` | the release bump | CI fails the run if it is empty |
| 17 | `docs/CHANGELOG.md` | the release entry | no |

### Conditional — up to 7 more

| File | Only when |
|------|-----------|
| `terraform/environments/production/secrets.tf` | the mainnet RPC carries an API key |
| `src/types.rs` | the chain settles a stablecoin with no `TokenType` yet |
| `scripts/stablecoin_matrix.py` | same — add the symbol to the allow-list |
| `static/{token}.png` + `ICONO_DE_TOKEN` in `static/x402.js` | same |
| `src/upto/types.rs` (`UPTO_DEPLOYED_NETWORKS`, `:60`) | the Permit2 proxy is genuinely deployed there — verify with `eth_getCode` against two independent RPCs, a wrong entry reports success while moving zero tokens |
| `src/payment_operator/addresses.rs` (`ESCROW_NETWORKS`, `:185`) | the chain joins escrow — also bump the `len() == 11` assertion at `:452` and the landing's escrow heading, EN + ES |
| `src/erc8004/mod.rs` (`supported_networks()`) | the chain joins ERC-8004 — also the landing stat card, EN + ES |

**Not** in the inventory, against a common assumption:

- `src/caip2.rs` — generic over `eip155:<chain-id>` (`Caip2NetworkId::eip155`,
  `:175`), no per-network table. Its diff in the last alta was pure `rustfmt`. It
  only changes for a new *family* (a new `Namespace`, `:52`).
- `static/networks.html` — the whole table is built from `GET /supported` at page
  load. Adding a row by hand is exactly what that page exists to avoid.

**Total**: 17 files always, up to 24 with the conditional ones — roughly 500-700
changed lines plus 1-2 PNGs, Terraform/Secrets Manager config and wallet funding.

## Success criteria

**The one criterion is `/supported`. "It compiles" is not a criterion, and
neither is "the deploy went green".**

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[].network] | map(select(startswith("newchain")))'
# [] means NOT DONE, whatever else is green
```

### Why: three networks that compile and are served nowhere

`Sei`, `SeiTestnet` and `XdcMainnet` are declared in the `Network` enum and wired
into every compiler-enforced site — `Display` (`src/network.rs:161`, `:174`,
`:175`), `FromStr` (`:223`, `:236`, `:237`), `to_caip2`, `from_caip2`,
`NetworkFamily` (`:293`, `:306`, `:307`). They compile. They serialize. Both
spellings of each resolve back to the right variant.

They are in **zero of the four `variants()` copies**. Measured on `dc109511`:

| | count |
|---|---|
| `Network` enum variants | 42 |
| union of the four `variants()` copies | 39 |
| in the enum, in no copy | 3 — `Sei`, `SeiTestnet`, `XdcMainnet` |
| distinct v1 names in live `/supported` | 39 |
| the union vs live `/supported` | identical, name for name |

That last row is the mechanism: `ProviderCache::from_env` iterates
`Network::variants()` (`src/provider_cache.rs:113`) and `/supported` walks the
provider map (`src/facilitator_local.rs:289`). A variant outside `variants()` gets
no provider and is advertised nowhere — with no error, no warning and no failing
test. The same silence covers a missing `RPC_URL_*` in the task definition (§2.3).

Reproduce it:

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l   # 39
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '[.kinds[].network]|unique|.[]' | grep -iE 'sei|xdc'    # nothing
```

`/supported` lists each chain twice — once by v1 name, once by CAIP-2 alias — so
an unfiltered `length` counts identifier strings, not networks. Filter out `:` for
a v1-name count, and use `python scripts/verify_landing_canonical.py` for the
canonical mainnet count.

### The rest of the checklist

- [ ] Both mainnet and testnet appear in `/supported`
- [ ] The new network is in all four `variants()` copies, not just the first
- [ ] `RPC_URL_*` for both networks reaches the container via Terraform
- [ ] Logo accessible at `/{network}.png` with HTTP 200
- [ ] `ICONO_DE_RED` in `static/x402.js` has all four keys, so `/networks` shows
      the logo instead of a monogram
- [ ] Network cards display on the landing page with the correct styling
- [ ] Balances load for both mainnet and testnet
- [ ] `python scripts/verify_landing_canonical.py` passes
- [ ] `/version` matches the `VERSION` you pushed
- [ ] `docs/CHANGELOG.md` entry written
- [ ] Both wallets funded with native tokens
- [ ] No compilation errors or warnings

---

**Document version**: 2.0 (2026-09-16)
**Originally based on**: Unichain integration (v1.3.4-v1.3.5)
**File inventory and deploy path re-measured against**: Robinhood Chain
(`7dbe194e`, 2026-07-20) and `.github/workflows/ci.yaml` at `dc109511` (2.29.6)
**Last updated**: 2026-09-16
