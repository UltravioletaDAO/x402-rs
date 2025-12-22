---
name: stablecoin-addition
description: Add new EIP-3009 compatible stablecoins to the x402-rs facilitator. This skill should be used when adding a new stablecoin (like USDT, EURC, AUSD) to supported networks. It covers EIP-3009 verification, backend implementation, frontend integration, testing, and deployment. Invoke this skill when the user wants to add a new stablecoin token.
---

# Stablecoin Addition Skill

This skill provides a complete workflow for adding new EIP-3009 compatible stablecoins to the x402-rs payment facilitator.

## When to Use This Skill

Invoke this skill when:
- Adding a new stablecoin token (USDT, EURC, AUSD, PYUSD, etc.)
- Expanding an existing stablecoin to new networks
- Verifying if a stablecoin supports EIP-3009 `transferWithAuthorization`

## Critical Requirements

**EIP-3009 is REQUIRED for x402 protocol.** A stablecoin must implement `transferWithAuthorization` for gasless meta-transactions. EIP-2612 `permit` is NOT sufficient (it only authorizes approvals, requiring a second transaction).

## Workflow Decision Tree

```
User wants to add stablecoin
         │
         ▼
┌────────────────────────────┐
│ 1. VERIFY EIP-3009 SUPPORT │  ← MANDATORY first step
└────────────────────────────┘
         │
    Does contract have
    transferWithAuthorization?
         │
    ┌────┴────┐
    │         │
   YES        NO
    │         │
    ▼         ▼
Continue   STOP - Token not
           compatible with x402
         │
         ▼
┌────────────────────────────┐
│ 2. GET EIP-712 METADATA    │
│    - name field            │
│    - version field         │
│    - decimals              │
└────────────────────────────┘
         │
         ▼
┌────────────────────────────┐
│ 3. BACKEND IMPLEMENTATION  │
│    - src/types.rs          │
│    - src/network.rs        │
│    - src/chain/evm.rs      │
└────────────────────────────┘
         │
         ▼
┌────────────────────────────┐
│ 4. FRONTEND INTEGRATION    │
│    - static/index.html     │
│    - TOKEN_SUPPORT         │
│    - TOKEN_INFO            │
│    - CSS styling           │
└────────────────────────────┘
         │
         ▼
┌────────────────────────────┐
│ 5. BUILD & TEST            │
│    - cargo build           │
│    - cargo test            │
│    - Local verification    │
└────────────────────────────┘
         │
         ▼
┌────────────────────────────┐
│ 6. DEPLOY                  │
│    - Version bump          │
│    - Docker build          │
│    - ECR push              │
│    - ECS update            │
└────────────────────────────┘
```

---

## Step 1: Verify EIP-3009 Support

**CRITICAL:** Always verify EIP-3009 support BEFORE implementing. Many stablecoins only implement EIP-2612.

### Quick Verification Method

Use `cast` or curl to check if `transferWithAuthorization` exists:

```bash
# Method 1: Call with dummy params - look for "invalid signature" vs "execution reverted"
cast call <CONTRACT_ADDRESS> \
  "transferWithAuthorization(address,address,uint256,uint256,uint256,bytes32,bytes)" \
  0x0000000000000000000000000000000000000001 \
  0x0000000000000000000000000000000000000002 \
  1000000 \
  0 \
  9999999999 \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  0x0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000 \
  --rpc-url <RPC_URL>
```

**Interpretation:**
- `"ECRecover: invalid signature"` or similar = EIP-3009 EXISTS (function found, needs valid sig)
- `"execution reverted"` (generic) = EIP-3009 NOT SUPPORTED (function doesn't exist)

### Use the Verification Script

Run the bundled verification script:

```bash
python3 .claude/skills/stablecoin-addition/scripts/verify_eip3009.py \
  --contract <CONTRACT_ADDRESS> \
  --rpc <RPC_URL>
```

### Common Stablecoins and EIP-3009 Status

| Stablecoin | EIP-3009 Support | Notes |
|------------|------------------|-------|
| USDC (Circle) | YES | Supported on all networks |
| EURC (Circle) | YES | EUR stablecoin |
| USDT (Legacy) | NO | Original Tether contract |
| USDT0 (Upgraded) | YES | New LayerZero OFT version |
| PYUSD (PayPal) | YES | Uses v,r,s signature format |
| AUSD (Agora) | YES | Agora dollar |
| GHO (Aave) | NO | Only EIP-2612 permit |
| crvUSD (Curve) | NO | Only EIP-2612 permit |
| DAI | NO | Uses permit, not transferWithAuthorization |
| Mento (cUSD, cCOP) | NO | Only EIP-2612 permit |

---

## Step 2: Get EIP-712 Metadata

For signature verification, the EIP-712 domain must match exactly.

### Required Fields

```solidity
EIP712Domain {
    string name,      // e.g., "USD Coin", "PayPal USD", "USD₮0"
    string version,   // e.g., "1" or "2"
    uint256 chainId,  // Network chain ID
    address verifyingContract  // Token contract address
}
```

### Query the Contract

```bash
# Get token name (usually matches EIP-712 name, but verify!)
cast call <CONTRACT> "name()" --rpc-url <RPC>

# Get version (if exposed)
cast call <CONTRACT> "version()" --rpc-url <RPC>

# Get decimals
cast call <CONTRACT> "decimals()" --rpc-url <RPC>

# Get DOMAIN_SEPARATOR (for verification)
cast call <CONTRACT> "DOMAIN_SEPARATOR()" --rpc-url <RPC>
```

### Alternative: Check Block Explorer

1. Navigate to contract on Etherscan/Arbiscan/etc.
2. Go to "Read Contract" tab
3. Find `name()`, `version()`, `DOMAIN_SEPARATOR()`

### Record the Metadata

Create a record for each network:

```
Network: Arbitrum
Contract: 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9
EIP-712 Name: "USD₮0"
EIP-712 Version: "1"
Decimals: 6
```

---

## Step 3: Backend Implementation

### 3.1 Update `src/types.rs`

Add new token type to the `TokenType` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TokenType {
    #[default]
    #[serde(rename = "usdc")]
    Usdc,
    // ... existing tokens ...
    /// New Token Name - X decimals
    #[serde(rename = "newtoken")]
    NewToken,
}
```

Add all required method implementations:

| Method | Purpose | Example Value |
|--------|---------|---------------|
| `decimals()` | Token decimals | `6` |
| `symbol()` | Short symbol | `"USDT"` |
| `display_name()` | Human name | `"Tether USD"` |
| `currency_symbol()` | Fiat symbol | `"$"` or `"EUR"` |
| `is_fiat_backed()` | Fiat backing | `true` |
| `all()` | Token array | Add to list |
| `eip712_name()` | EIP-712 name | From Step 2 |
| `eip712_version()` | EIP-712 version | From Step 2 |
| `FromStr` | Parse from string | Match lowercase |

Also update the `TokenTypeParseError` message.

### 3.2 Update `src/network.rs`

Add token deployment constants for each supported network:

```rust
// ============================================================================
// NEWTOKEN Deployments
// ============================================================================

static NEWTOKEN_NETWORK: Lazy<NEWTOKENDeployment> = Lazy::new(|| {
    NEWTOKENDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x...").into(),
            network: Network::NetworkName,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "EIP-712 Name Here".into(),
            version: "1".into(),
        }),
    })
});

/// Wrapper struct for the new token
#[derive(Clone, Debug)]
pub struct NEWTOKENDeployment(pub TokenDeployment);

impl Deref for NEWTOKENDeployment {
    type Target = TokenDeployment;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl NEWTOKENDeployment {
    pub fn by_network<N: Borrow<Network>>(network: N) -> Option<&'static NEWTOKENDeployment> {
        match network.borrow() {
            Network::NetworkName => Some(&NEWTOKEN_NETWORK),
            _ => None,
        }
    }

    pub fn supported_networks() -> &'static [Network] {
        &[Network::NetworkName]
    }
}
```

Update `get_token_deployment()` and `supported_networks_for_token()` functions.

### 3.3 Update `src/chain/evm.rs`

Add import for the new deployment struct and update `find_known_eip712_metadata()`:

```rust
// Check NewToken
if let Some(newtoken) = NEWTOKENDeployment::by_network(network) {
    if newtoken.address() == asset_mixed {
        if let Some(eip712) = &newtoken.eip712 {
            return Some((eip712.name.clone(), eip712.version.clone()));
        }
    }
}
```

### Special Case: v,r,s Signature Format

If the token uses v,r,s format (like PYUSD) instead of compact signature:

1. Add the token to `needs_split_signature()` function
2. Test with v,r,s format in payload

---

## Step 4: Frontend Integration

### 4.1 Update TOKEN_SUPPORT

In `static/index.html`, add the token to supported networks:

```javascript
const TOKEN_SUPPORT = {
    'network-mainnet': ['usdc', 'newtoken'],  // Add 'newtoken'
    // ...
};
```

### 4.2 Update TOKEN_INFO

```javascript
const TOKEN_INFO = {
    'usdc': { name: 'USDC', decimals: 6 },
    'newtoken': { name: 'NEWTOKEN', decimals: 6 },  // Add
};
```

### 4.3 Add CSS Styling

```css
/* Token pill styling */
.token-pill.newtoken {
    background: #HEXCOLOR;  /* Brand color */
    color: white;
}
```

### Common Token Colors

| Token | Hex Color | Description |
|-------|-----------|-------------|
| USDC | #2775CA | Circle blue |
| EURC | #4E9F3D | Euro green |
| USDT | #50AF95 | Tether green |
| PYUSD | #0070BA | PayPal blue |
| AUSD | #FF6B6B | Agora coral |

---

## Step 5: Build and Test

### Compile

```bash
cargo build --release
cargo clippy --all-targets --all-features
cargo test
```

### Local Testing

```bash
# Start local facilitator
RUST_LOG=debug cargo run --release

# Check /supported endpoint
curl http://localhost:8080/supported | jq '.kinds[] | select(.network == "network-name")'

# Verify token appears in response
curl http://localhost:8080/supported | jq '.kinds[].extra.tokens[] | select(.token == "newtoken")'
```

### Integration Test

```bash
cd tests/integration
python test_usdc_payment.py --network network-name --token newtoken
```

---

## Step 6: Deploy

### Version Bump

```bash
# Check current deployed version
curl -s https://facilitator.ultravioletadao.xyz/version

# Update Cargo.toml version
# Build to update Cargo.lock
cargo build --release
```

### Docker Build

```bash
docker build --platform linux/amd64 \
  --build-arg FACILITATOR_VERSION=vX.Y.Z \
  -t facilitator:vX.Y.Z .
```

### Push to ECR and Deploy

Use the `/deploy-prod` skill or manually:

```bash
# Tag and push
docker tag facilitator:vX.Y.Z 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:vX.Y.Z
aws ecr get-login-password --region us-east-2 | docker login --username AWS --password-stdin 518898403364.dkr.ecr.us-east-2.amazonaws.com
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:vX.Y.Z

# Update ECS service
aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2
```

### Verify Deployment

```bash
# Check version
curl https://facilitator.ultravioletadao.xyz/version

# Check token support
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.extra.tokens[].token == "newtoken")'

# Run /test-prod skill
```

---

## Checklist Summary

### Pre-Implementation
- [ ] Verify EIP-3009 support using verification script
- [ ] Gather EIP-712 metadata (name, version, decimals)
- [ ] Document contract addresses per network

### Backend (src/)
- [ ] Add enum variant to `TokenType` in `types.rs`
- [ ] Implement all TokenType methods
- [ ] Add deployment constants in `network.rs`
- [ ] Add deployment struct with `by_network()` and `supported_networks()`
- [ ] Update `get_token_deployment()` function
- [ ] Update `supported_networks_for_token()` function
- [ ] Add EIP-712 lookup in `chain/evm.rs`
- [ ] Handle special signature format if needed

### Frontend (static/)
- [ ] Add token to TOKEN_SUPPORT for each network
- [ ] Add token to TOKEN_INFO
- [ ] Add CSS styling with brand color

### Testing
- [ ] `cargo build --release` succeeds
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes
- [ ] Local /supported endpoint shows token
- [ ] Integration tests pass

### Deployment
- [ ] Version bumped in Cargo.toml
- [ ] Docker image built with version arg
- [ ] Image pushed to ECR
- [ ] ECS service updated
- [ ] Production /version shows correct version
- [ ] Production /supported shows new token
- [ ] Run /test-prod for full verification

---

## Resources

This skill includes helper resources:

### scripts/verify_eip3009.py

Python script to verify EIP-3009 support on a contract. Run:

```bash
python3 .claude/skills/stablecoin-addition/scripts/verify_eip3009.py \
  --contract 0xCONTRACT_ADDRESS \
  --rpc https://rpc-url
```

### references/eip3009_verification.md

Detailed documentation on EIP-3009 verification methods, common pitfalls, and troubleshooting.

---

## Troubleshooting

### "Invalid signature" errors

- EIP-712 name doesn't match contract exactly
- EIP-712 version is wrong (try "1" vs "2")
- Contract uses different signature format (v,r,s vs compact)

### Token not appearing in /supported

- Check `get_token_deployment()` returns the deployment
- Verify `supported_networks_for_token()` includes the network
- Check RPC URL is configured for the network

### Frontend not showing token badge

- Verify TOKEN_SUPPORT includes the token for that network
- Check TOKEN_INFO has the token entry
- Clear browser cache

### Signature format issues

If you see v,r,s in the signature (65 bytes with v as first byte):
1. Add token to `needs_split_signature()` in `chain/evm.rs`
2. Use `split_signature()` helper function
