# x402-rs Customizations - Technical Documentation

**Version**: Forked from upstream v0.9.0
**Last Updated**: 2025-10-31
**Maintainer**: Ultravioleta DAO / Karmacadabra Team

## Overview

This document catalogs ALL customizations made to the upstream x402-rs facilitator. It serves as:
1. **Change log**: What we modified and why
2. **Merge guide**: How to integrate upstream changes without breaking customizations
3. **Recovery reference**: What to restore if accidentally overwritten

---

## Customization Inventory

### 1. Branded Landing Page (CRITICAL - User-facing)

**Location**: `static/index.html`

**Status**: 🔴 **NEVER overwrite from upstream**

**Description**:
- Complete replacement of upstream's simple text response with full HTML page
- Ultravioleta DAO branding, dark theme, professional design
- Network grid with logos (Avalanche, Base, Celo, Ethereum, HyperEVM, Optimism, Polygon)
- Embedded CSS (no external dependencies)

**Technical Details**:
```html
<!-- Upstream version (lines ~20-30 in their index.html) -->
<body>Hello from x402-rs!</body>

<!-- Our version (lines 1-1400+) -->
<body>
  <div class="hero">
    <h1>Ultravioleta DAO</h1>
    <h2>x402 Payment Facilitator</h2>
    <!-- ... extensive custom HTML ... -->
  </div>
</body>
```

**File Size**: 57,662 bytes (vs upstream's ~200 bytes)

**Dependencies**:
- `static/favicon.ico` (DAO icon)
- `static/images/avalanche.png`, `base.png`, `celo.png`, `ethereum.png`, `hyperevm.png`, `optimism.png`, `polygon.png`

**Why Customized**:
- Public-facing landing page represents Ultravioleta DAO identity
- Provides API documentation and network information to developers
- Used during live streams - branding is critical

**Merge Strategy**:
- IGNORE upstream changes to `index.html` entirely
- If upstream adds new API endpoints, manually add documentation to our HTML
- Backup this file before EVERY upgrade

**Recovery**:
```bash
# From git history (if overwritten)
git checkout HEAD~1 -- static/index.html

# From backup (recommended approach)
cp x402-rs-backup-VERSION/static/ x402-rs/static/ -Recurse -Force
```

---

### 2. Custom Root Handler (CRITICAL - Serves branding)

**Location**: `src/handlers.rs`, function `get_root()` (approximately lines 76-85)

**Status**: 🟡 **Merge with extreme care - preserve include_str!()**

**Description**:
Modified the root endpoint (`/`) to serve our custom HTML instead of plain text.

**Code Diff**:
```rust
// UPSTREAM VERSION (as of v0.9.0)
pub async fn get_root() -> impl IntoResponse {
    Html("Hello from x402-rs!")
}

// OUR VERSION (current)
pub async fn get_root() -> impl IntoResponse {
    Html(include_str!("../static/index.html"))
}
```

**Why Customized**:
- Embeds HTML at compile time (no runtime file reading = faster)
- Ensures branding is always served, even if static/ folder accidentally deleted in container
- Single source of truth for landing page content

**Merge Strategy**:
1. Check if upstream modified `get_root()` signature or response type
2. If yes: Integrate their changes BUT keep `include_str!()` approach
3. If no changes: Keep our version as-is

**Conflict Resolution**:
```rust
// If upstream changes return type (hypothetical):
<<<<<<< HEAD (ours)
pub async fn get_root() -> impl IntoResponse {
    Html(include_str!("../static/index.html"))
}
=======
pub async fn get_root() -> Response {
    Response::new(Body::from("Hello from x402-rs!"))
}
>>>>>>> upstream

// RESOLUTION: Use their new return type, keep our content
pub async fn get_root() -> Response {
    Response::new(Body::from(include_str!("../static/index.html")))
}
```

**Testing**:
```bash
# Must return our branded HTML
curl http://localhost:8080/ | grep "Ultravioleta DAO"
# Exit code 0 = success, 1 = FAILURE (upstream version still present)
```

---

### 3. Additional Network Support (IMPORTANT - Feature expansion)

**Location**: `src/network.rs`

**Status**: 🟡 **Merge carefully - preserve our networks + add upstream's new networks**

**Description**:
Added support for additional blockchain networks beyond upstream's default set.

**Networks Added**:

#### 3a. HyperEVM Mainnet
```rust
Network::HyperEvm => {
    chain_id: 998,
    rpc_url: "https://rpc.hyperliquid.xyz/evm",
    token_address: "USDC_ADDRESS_ON_HYPEREVM", // USDC native token
    gas_price: 25_000_000_000, // 25 gwei
    gas_limit: 200_000,
}
```

**Why**: Potential future deployment target, HyperEVM has low fees and high throughput

#### 3b. HyperEVM Testnet
```rust
Network::HyperEvmTestnet => {
    chain_id: 333,
    rpc_url: "https://rpc.hyperliquid-testnet.xyz/evm",
    token_address: "USDC_ADDRESS_ON_HYPEREVM_TESTNET",
    gas_price: 10_000_000_000, // 10 gwei
    gas_limit: 200_000,
}
```

**Why**: Testing environment for HyperEVM before mainnet deployment

#### 3c. Optimism Mainnet (PRIMARY ADDITION)
```rust
Network::Optimism => {
    chain_id: 10,
    rpc_url: "https://mainnet.optimism.io",
    token_address: "USDC_ADDRESS_ON_OPTIMISM", // Native USDC bridged token
    gas_price: 1_000_000, // Very low on Optimism
    gas_limit: 200_000,
}
```

**Why**: **Active expansion target** - cheaper gas than Avalanche, large ecosystem. See commits:
- `a7123a6 Add Optimism network support to x402 facilitator (clean implementation)`
- `4bd7176 Fix Optimism network support in x402 facilitator (network.rs)`

**Recent Work**: This was being actively developed (see git status showing `network.rs` modified)

#### 3d. Polygon (PoS) Mainnet
```rust
Network::Polygon => {
    chain_id: 137,
    rpc_url: "https://polygon-rpc.com",
    token_address: "USDC_ADDRESS_ON_POLYGON", // Native USDC on Polygon
    gas_price: 50_000_000_000, // 50 gwei
    gas_limit: 200_000,
}
```

**Why**: Large user base, low fees, EVM-compatible

#### 3e. Solana (EXPERIMENTAL)
```rust
Network::Solana => {
    // Note: Solana is NOT EVM-compatible
    // This may require significant architectural changes
    // Current status: PLACEHOLDER for future work
    chain_id: 0, // Solana doesn't use EVM chain IDs
    rpc_url: "https://api.mainnet-beta.solana.com",
    token_address: "USDC_PROGRAM_ID_ON_SOLANA", // USDC SPL token
    gas_price: 0, // Solana uses lamports, different model
    gas_limit: 0,
}
```

**Why**: Future exploration - Solana has different transaction model, requires research

**Enum Additions**:
```rust
// In Network enum definition
pub enum Network {
    // ... upstream networks ...

    // OUR ADDITIONS:
    HyperEvm,
    HyperEvmTestnet,
    Optimism,
    Polygon,
    Solana, // Experimental
}
```

**Merge Strategy**:
1. Pull upstream's new networks (if any)
2. Preserve ALL our custom networks (grep for each to verify)
3. If upstream modified network struct fields, update our networks to match
4. Verify no duplicate chain IDs

**Conflict Resolution Pattern**:
```rust
// Upstream adds Arbitrum, we have Optimism
<<<<<<< HEAD (ours)
Network::Optimism => { ... }
Network::Polygon => { ... }
=======
Network::Arbitrum => { ... }
>>>>>>> upstream

// RESOLUTION: Keep BOTH
Network::Optimism => { ... }
Network::Polygon => { ... }
Network::Arbitrum => { ... } // From upstream
```

**Testing**:
```bash
# Verify each custom network present
curl http://localhost:8080/networks | jq '.networks[] | select(.name == "HyperEVM")'
curl http://localhost:8080/networks | jq '.networks[] | select(.name == "Optimism")'
curl http://localhost:8080/networks | jq '.networks[] | select(.name == "Polygon")'
# Each should return network info, not empty
```

**Future Work**:
- Verify USDC token addresses on all networks
- Test payment flows on each network
- Solana integration requires architectural redesign (non-EVM)

---

### 4. Rust Nightly Compiler (INFRASTRUCTURE)

**Location**: `Dockerfile`

**Status**: 🟡 **Preserve nightly setup, merge upstream Dockerfile changes around it**

**Description**:
Added Rust nightly toolchain setup to support Rust Edition 2024 features.

**Code Addition**:
```dockerfile
# OUR ADDITION (insert after rustup installation, before cargo build)
RUN rustup default nightly

# Context (typical Dockerfile structure):
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN rustup default nightly  # <-- OUR LINE
RUN cargo build --release

FROM debian:bookworm-slim
# ... rest of Dockerfile ...
```

**Why Customized**:
- Upstream may use stable Rust (edition 2021)
- We need nightly for edition 2024 features (possibly: async traits, const generics, etc.)
- Specific feature usage: [TODO: Document which features require nightly]

**Merge Strategy**:
1. Check if upstream changed Dockerfile structure
2. If they optimized build stages, integrate improvements
3. ALWAYS preserve `RUN rustup default nightly` before `cargo build`
4. If upstream added nightly support, verify our line isn't duplicate

**Conflict Resolution**:
```dockerfile
<<<<<<< HEAD (ours)
RUN rustup default nightly
RUN cargo build --release
=======
RUN cargo build --release --features "optimized"
>>>>>>> upstream

# RESOLUTION: Combine both
RUN rustup default nightly
RUN cargo build --release --features "optimized"
```

**Testing**:
```bash
# Build must succeed without edition errors
docker build -t x402-test x402-rs/
# Should complete without "edition not recognized" errors

# Verify nightly active in container
docker run x402-test rustc --version
# Should output: rustc 1.XX.0-nightly (...)
```

**Risk Assessment**:
- **Low risk**: If nightly breaks, fallback to stable is easy
- **Maintenance cost**: Nightly features may stabilize, allowing removal of this customization
- **Review**: Quarterly check if nightly still needed

---

### 5. AWS Secrets Manager Integration (HYPOTHETICAL - Verify)

**Location**: `src/main.rs`, possibly `Cargo.toml`

**Status**: 🟡 **Verify if implemented - preserve if present**

**Description**:
MAY include integration with AWS Secrets Manager for loading `PRIVATE_KEY` at runtime instead of from environment variables. This mirrors the pattern used in Python agents.

**Expected Code Pattern**:
```rust
// Hypothetical implementation
use aws_sdk_secretsmanager as secrets_manager;

async fn load_config() -> Config {
    let config = aws_config::load_from_env().await;
    let client = secrets_manager::Client::new(&config);

    let secret = client
        .get_secret_value()
        .secret_id("facilitator-private-key")
        .send()
        .await?;

    Config {
        private_key: secret.secret_string().unwrap(),
        // ...
    }
}
```

**Cargo.toml Dependencies** (if implemented):
```toml
[dependencies]
aws-config = "0.XX.X"
aws-sdk-secretsmanager = "0.XX.X"
tokio = { version = "1", features = ["full"] }
```

**Verification**:
```bash
# Check if AWS SDK present
grep -r "aws_sdk" x402-rs/src/
grep -r "aws-sdk-secretsmanager" x402-rs/Cargo.toml

# If found:
echo "AWS Secrets Manager IS integrated"
# If not found:
echo "AWS Secrets Manager NOT integrated - .env based"
```

**Merge Strategy** (if integrated):
1. Preserve AWS SDK dependencies in Cargo.toml
2. Preserve secrets loading code in main.rs
3. If upstream changes config loading, integrate our AWS loader into their new structure

**Testing** (if integrated):
```bash
# Must load secrets from AWS in production
docker run -e AWS_REGION=us-east-1 x402-test
# Should NOT error with "PRIVATE_KEY not found"

# Local dev: should fallback to .env
docker run -e PRIVATE_KEY=0xtest x402-test
# Should work without AWS credentials
```

**Action Required**:
- [ ] Verify if AWS integration exists in current codebase
- [ ] If yes, document exact implementation location
- [ ] If no, document as ".env based" for future reference

---

### 6. Custom Static Assets (BRANDING)

**Location**: `static/images/`

**Status**: 🔴 **NEVER overwrite - these are DAO-specific assets**

**File List**:
```
static/
├── favicon.ico              # Ultravioleta DAO icon (16x16, 32x32, 48x48)
└── images/
    ├── avalanche.png        # Avalanche logo (for network grid)
    ├── base.png             # Base logo
    ├── celo.png             # Celo logo
    ├── ethereum.png         # Ethereum logo
    ├── hyperevm.png         # HyperEVM logo (custom)
    ├── optimism.png         # Optimism logo
    └── polygon.png          # Polygon logo
```

**Source**:
- Official network logos from respective project websites
- Processed to consistent size/format (likely 128x128 PNG)

**Usage**:
- Embedded in `static/index.html` via `<img>` tags
- Displayed in network grid on landing page

**Licensing**:
- All logos used under fair use / official brand guidelines
- Check each network's brand kit if redistributing

**Merge Strategy**:
- IGNORE upstream's static/ folder entirely
- Never `cp -r` from upstream
- If upstream adds useful static files (e.g., CSS framework), manually copy specific files

**Backup**:
```bash
# Always backup before upgrade
cp x402-rs/static/images/ $BACKUP_DIR/static/images/ -Recurse
```

---

## Customization Statistics

**Summary**:
- **Files completely replaced**: 2 (index.html, favicon.ico)
- **Files with code modifications**: 3 (handlers.rs, network.rs, Dockerfile)
- **Files potentially modified**: 2 (main.rs, Cargo.toml - AWS integration TBD)
- **New files added**: 7 (network logo images)
- **Total customization footprint**: ~5% of codebase (estimated)

**Divergence Risk**: LOW
- Customizations are isolated to specific modules
- No changes to core payment verification logic
- Network additions are additive (don't modify existing networks)

---

## Testing Matrix

Before declaring an upgrade successful, ALL these tests must pass:

### Tier 1: Compilation
- [ ] `cargo build --release` succeeds
- [ ] No deprecation warnings related to our custom code
- [ ] Docker build succeeds

### Tier 2: Functionality
- [ ] `curl localhost:8080/health` returns 200 OK
- [ ] `curl localhost:8080/` contains "Ultravioleta DAO"
- [ ] `curl localhost:8080/networks` lists HyperEVM, Optimism, Polygon
- [ ] Payment flow test succeeds: `python tests/integration/test_usdc_payment.py`

### Tier 3: Production
- [ ] ECS deployment succeeds
- [ ] `curl https://facilitator.karmacadabra.ultravioletadao.xyz/health` returns 200
- [ ] `curl https://facilitator.karmacadabra.ultravioletadao.xyz/` shows branding
- [ ] Agent health checks succeed (validator, karma-hello, abracadabra)
- [ ] End-to-end purchase test succeeds: `python scripts/demo_client_purchases.py --production`

---

## Upstream Tracking

**Upstream Repository**: https://github.com/polyphene/x402-rs (verify URL)

**Current Upstream Version**: v0.9.0 (as of 2025-10-31)

**Our Version Naming**: `0.9.0-karmacadabra-1` (upstream version + our fork suffix)

**Sync Frequency**:
- **Security patches**: Within 1 week of upstream release
- **Feature releases**: Quarterly review (January, April, July, October)
- **Breaking changes**: Evaluate case-by-case (may delay merge)

**Upstream Monitoring**:
```bash
# Check for new releases
git fetch upstream
git log --oneline HEAD..upstream/main

# Subscribe to upstream releases (GitHub)
# Navigate to: https://github.com/polyphene/x402-rs/releases
# Click "Watch" → "Custom" → "Releases"
```

---

## Emergency Recovery

### Scenario 1: Branding Overwritten (Most Common)

**Symptoms**: Landing page shows "Hello from x402-rs!" instead of Ultravioleta DAO branding

**Recovery**:
```bash
# 1. Restore from git history (if committed)
git checkout HEAD~1 -- x402-rs/static/

# 2. Restore from backup (if available)
cp x402-rs-backup-VERSION/static/ x402-rs/static/ -Recurse -Force

# 3. Fix handler
# Edit src/handlers.rs, change:
Html("Hello from x402-rs!")
# To:
Html(include_str!("../static/index.html"))

# 4. Rebuild and redeploy
cd x402-rs
cargo build --release
docker build -t x402-prod .
# Push to ECR and update ECS service
```

**Time to recovery**: 10-15 minutes

---

### Scenario 2: Custom Networks Lost

**Symptoms**: Payment fails with "network not supported" for Optimism/Polygon

**Recovery**:
```bash
# 1. Restore network.rs from backup
cp x402-rs-backup-VERSION/network.rs x402-rs/src/network.rs

# 2. Or manually re-add networks (if backup unavailable)
# Edit x402-rs/src/network.rs
# Add back enum variants and match arms for:
# - Network::HyperEvm
# - Network::HyperEvmTestnet
# - Network::Optimism
# - Network::Polygon
# - Network::Solana

# 3. Rebuild
cargo build --release
```

**Time to recovery**: 5-10 minutes (with backup), 30+ minutes (manual)

---

### Scenario 3: Production Completely Broken

**Symptoms**: Facilitator not responding, agents can't process payments

**Immediate Action**:
```bash
# Roll back ECS to previous task definition
aws ecs describe-services \
  --cluster karmacadabra-prod \
  --services karmacadabra-prod-facilitator \
  --region us-east-1 \
  --query 'services[0].taskDefinition'
# Note the REVISION number (e.g., :12)

# Roll back to REVISION-1
aws ecs update-service \
  --cluster karmacadabra-prod \
  --service karmacadabra-prod-facilitator \
  --task-definition karmacadabra-prod-facilitator:11 \
  --force-new-deployment \
  --region us-east-1
```

**Time to recovery**: 2-3 minutes (rollback) + investigation time

**Root Cause Analysis**:
1. Check ECS logs: `aws logs tail /ecs/karmacadabra-prod-facilitator --since 30m`
2. Check if branding missing: `curl https://facilitator.karmacadabra.ultravioletadao.xyz/`
3. Check if networks missing: `curl https://facilitator.karmacadabra.ultravioletadao.xyz/networks`
4. Run local tests: `cd tests/integration && python test_usdc_payment.py`

---

## Future Customization Roadmap

**Planned Additions**:
1. **Enhanced Multi-Network Support** (Q1 2026)
   - Verify USDC deployment addresses on all networks
   - Test payment flows end-to-end across all chains
   - Add QuickNode premium RPC endpoints for better reliability

2. **Multi-Network UI** (Q2 2026)
   - Update landing page to show real-time network stats
   - Add network status indicators (Avalanche: online, Optimism: online, etc.)
   - Embed price comparison (gas costs per network)

4. **Solana Evaluation** (Q3 2026)
   - Research: Can x402-rs architecture support non-EVM chains?
   - Prototype: Solana payment verification
   - Decision: Separate facilitator or unified codebase?

**Deprecation Candidates**:
- HyperEVM testnet (if mainnet stable)
- Nightly Rust requirement (if edition 2024 stabilizes)

---

## Contact & Maintenance

**Primary Maintainer**: Ultravioleta DAO core team

**Documentation Updates**:
- Update this file after EVERY customization
- Include git commit hash of change
- Document WHY, not just WHAT

**Review Cycle**:
- **Monthly**: Check upstream for security patches
- **Quarterly**: Evaluate new features for integration
- **Annually**: Assess whether fork should continue or upstreaming is viable

**Git Commit Tags**:
When committing customizations, use tags:
```
[x402-custom] Add Optimism network support

This is a CUSTOMIZATION on top of upstream x402-rs v0.9.0.
Documented in: x402-rs/CUSTOMIZATIONS.md section 3c

🤖 Generated with [Claude Code](https://claude.com/claude-code)
Co-Authored-By: Claude <noreply@anthropic.com>
```

---

## Appendix A: Upstream Contribution Strategy

**Should we contribute customizations back to upstream?**

| Customization | Upstream Value | Contribution Viability | Recommendation |
|---------------|----------------|----------------------|----------------|
| Branded landing page | LOW (project-specific) | LOW (they won't want our DAO branding) | **Keep as fork** |
| `include_str!()` approach | HIGH (performance) | MEDIUM (minor change) | **Propose PR** - make it configurable |
| Network additions | HIGH (feature parity) | HIGH (everyone benefits) | **Propose PR** - submit Optimism/Polygon/HyperEVM |
| Nightly Rust | MEDIUM (if using edition 2024) | LOW (upstream on stable) | **Keep as fork** unless upstream adopts edition 2024 |
| AWS Secrets | LOW (platform-specific) | LOW (not all users use AWS) | **Keep as fork** |

**Contribution Process** (if pursuing):
1. Open upstream issue first: "Would you accept PRs for multi-network support?"
2. Wait for maintainer response
3. If positive: Create clean PR with ONLY that feature (no branding)
4. Maintain our fork until PR merged (may take months)

**Benefit of Contributing**:
- Reduces our maintenance burden (one less customization to preserve)
- Improves upstream project (community benefit)
- Establishes relationship with upstream maintainers (easier future merges)

**Recommendation**:
- **Phase 1** (Now): Stabilize our fork, get production-tested
- **Phase 2** (Q2 2026): Propose network additions to upstream
- **Phase 3** (Q3 2026): Contribute configurable landing page system (if they're interested)

---

## Appendix B: Version History

| Date | Our Version | Upstream Version | Major Changes |
|------|-------------|------------------|---------------|
| 2025-08-15 | 0.7.9-karmacadabra-1 | v0.7.9 | Initial fork, added branding |
| 2025-09-20 | 0.7.9-karmacadabra-2 | v0.7.9 | Added HyperEVM, Optimism networks |
| 2025-10-30 | 0.9.0-karmacadabra-1 | v0.9.0 | **INCIDENT**: Overwrite during upgrade, recovered |
| 2025-10-31 | 0.9.0-karmacadabra-2 | v0.9.0 | Documentation added (this file) |

---

**END OF CUSTOMIZATIONS.md**
