---
name: add-erc8004-network
description: Add ERC-8004 Trustless Agents support to new networks in the x402-rs facilitator. This skill should be used when adding ERC-8004 reputation support to networks (e.g., "add erc8004 polygon", "enable 8004 on arbitrum"). It handles backend integration, frontend updates, SDK updates, and deployment. Invoke when user mentions adding ERC-8004, reputation support, or trustless agents to a network.
---

# Add ERC-8004 Network Support

This skill adds ERC-8004 Trustless Agents reputation support to networks in the x402-rs facilitator ecosystem.

## Prerequisites

Before adding ERC-8004 support for a network:

1. **Verify ERC-8004 deployment** - Check https://github.com/erc-8004/erc-8004-contracts for deployed networks
2. **Verify facilitator support** - The network must already be supported by the facilitator (check `src/network.rs`)
3. **Get contract addresses** - ERC-8004 uses CREATE2 deterministic deployment:
   - **Mainnets**: Same addresses as Ethereum Mainnet
   - **Testnets**: Same addresses as Ethereum Sepolia

## Contract Addresses Reference

### Mainnet Addresses (CREATE2 Deterministic)
```
IdentityRegistry:   0x8004A169FB4a3325136EB29fA0ceB6D2e539a432
ReputationRegistry: 0x8004BAa17C55a88189AE136b182e5fdA19dE9b63
```

### Testnet Addresses
```
IdentityRegistry:   0x8004A818BFB912233c491871b3d84c89A494BD9e
ReputationRegistry: 0x8004B663056A597Dffe9eCcC1965A193B7388713
ValidationRegistry: 0x8004Cb1BF31DAf7788923b405b754f57acEB4272
```

## Implementation Steps

### Step 1: Update Facilitator Backend

Edit `src/erc8004/mod.rs`:

1. **Add contract constants** (if new address pattern):
   ```rust
   // Network Name - Official deployment (DATE)
   pub const NETWORK_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
       identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
       reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
       validation_registry: None, // or Some(...) for testnets
   };
   ```

2. **Update `get_contracts()` function** - Add match arm:
   ```rust
   Network::NetworkName => Some(NETWORK_CONTRACTS),
   ```

3. **Update `supported_networks()` function** - Add to vec:
   ```rust
   Network::NetworkName,
   ```

4. **Update `supported_network_names()` function** - Add string:
   ```rust
   "network-name",
   ```

5. **Add test** (optional but recommended):
   ```rust
   #[test]
   fn test_network_name_supported() {
       assert!(is_erc8004_supported(&Network::NetworkName));
       let contracts = get_contracts(&Network::NetworkName).unwrap();
       assert_eq!(contracts.identity_registry, MAINNET_CONTRACTS.identity_registry);
   }
   ```

### Step 2: Update Frontend

Edit `static/index.html`:

1. **Update ERC-8004 note text** - Find and update:
   ```html
   <p ... data-i18n="endpoints.erc8004Note">
       Supported networks: ethereum, ethereum-sepolia, base-mainnet, NEW-NETWORK.
   </p>
   ```

2. **Update English translation**:
   ```javascript
   "endpoints.erc8004Note": "Supported networks: ..., new-network. Uses official ERC-8004 contracts.",
   ```

3. **Update Spanish translation**:
   ```javascript
   "endpoints.erc8004Note": "Redes soportadas: ..., new-network. Usa contratos oficiales ERC-8004.",
   ```

4. **Add block explorer link** (optional) - For major networks:
   ```html
   <a href="https://EXPLORER/address/0x8004BAa17C55a88189AE136b182e5fdA19dE9b63" target="_blank"
      style="font-size: 0.7rem; color: var(--accent); ..."
      onmouseover="..." onmouseout="...">
       Network Contracts
   </a>
   ```

### Step 3: Update Python SDK

Edit `uvd-x402-sdk-python/src/uvd_x402_sdk/erc8004.py`:

1. **Update `Erc8004Network` type**:
   ```python
   Erc8004Network = Literal["ethereum", "ethereum-sepolia", "base-mainnet", "new-network"]
   ```

2. **Add to `ERC8004_CONTRACTS` dict**:
   ```python
   "new-network": Erc8004ContractAddresses(
       identity_registry="0x8004A169FB4a3325136EB29fA0ceB6D2e539a432",
       reputation_registry="0x8004BAa17C55a88189AE136b182e5fdA19dE9b63",
   ),
   ```

3. **Bump version** in `pyproject.toml`

### Step 4: Update TypeScript SDK

Edit `uvd-x402-sdk-typescript/src/backend/index.ts`:

1. **Add to `ERC8004_CONTRACTS` object**:
   ```typescript
   'new-network': {
     identityRegistry: '0x8004A169FB4a3325136EB29fA0ceB6D2e539a432',
     reputationRegistry: '0x8004BAa17C55a88189AE136b182e5fdA19dE9b63',
   },
   ```

2. **Update `Erc8004Network` type**:
   ```typescript
   export type Erc8004Network = 'ethereum' | 'ethereum-sepolia' | 'base-mainnet' | 'new-network';
   ```

3. **Bump version** in `package.json`

4. **Build SDK**: `npm run build`

### Step 5: Update Documentation

Edit `docs/CHANGELOG.md`:
- Add entry for the new ERC-8004 network support
- Include contract addresses table
- Reference the ERC-8004 spec

### Step 6: Build and Deploy

```bash
# Bump version in Cargo.toml
# Build and push Docker image
./scripts/build-and-push.sh vX.Y.Z

# Update ECS task definition
aws ecs register-task-definition --cli-input-json file:///tmp/task-def.json --region us-east-2

# Deploy to ECS
aws ecs update-service --cluster facilitator-production --service facilitator-production \
  --task-definition facilitator-production:NEW_REVISION --force-new-deployment --region us-east-2
```

### Step 7: Verify Deployment

```bash
# Check version
curl -s https://facilitator.ultravioletadao.xyz/version

# Check ERC-8004 supported networks
curl -s https://facilitator.ultravioletadao.xyz/feedback | jq '.supportedNetworks'

# Verify frontend text
curl -s https://facilitator.ultravioletadao.xyz/ | grep "new-network"
```

### Step 8: Commit and Push SDKs

```bash
# Python SDK
cd uvd-x402-sdk-python
git add src/uvd_x402_sdk/erc8004.py pyproject.toml
git commit -m "feat(erc8004): add NEW-NETWORK support"
git push origin main

# TypeScript SDK
cd uvd-x402-sdk-typescript
git add src/backend/index.ts package.json
git commit -m "feat(erc8004): add NEW-NETWORK support"
git push origin main
```

## Network Name Mapping

Use these exact network names from `src/network.rs`:

| Network | Rust Enum | String Name |
|---------|-----------|-------------|
| Ethereum | `Network::Ethereum` | `"ethereum"` |
| Ethereum Sepolia | `Network::EthereumSepolia` | `"ethereum-sepolia"` |
| Base | `Network::Base` | `"base-mainnet"` or `"base"` |
| Base Sepolia | `Network::BaseSepolia` | `"base-sepolia"` |
| Polygon | `Network::Polygon` | `"polygon"` |
| Polygon Amoy | `Network::PolygonAmoy` | `"polygon-amoy"` |
| Arbitrum | `Network::Arbitrum` | `"arbitrum"` |
| Arbitrum Sepolia | `Network::ArbitrumSepolia` | `"arbitrum-sepolia"` |
| Celo | `Network::Celo` | `"celo"` |
| Celo Sepolia | `Network::CeloSepolia` | `"celo-sepolia"` |
| BSC | `Network::Bsc` | `"bsc"` |
| Monad | `Network::Monad` | `"monad"` |
| Optimism | `Network::Optimism` | `"optimism"` |
| Avalanche | `Network::Avalanche` | `"avalanche"` |

## Batch Addition

When adding multiple networks at once:

1. Group mainnets and testnets separately
2. Use shared contract constants (mainnet vs testnet addresses)
3. Update all files in a single pass
4. Single version bump covers all additions
5. Single deployment for all changes

## Troubleshooting

- **Network not in facilitator**: Add network support first using `/add-network` skill
- **Different contract addresses**: Some networks may have custom deployments - verify on GitHub
- **Build errors**: Check `Network` enum matches in `get_contracts()` function
- **Frontend not updating**: Ensure `static/index.html` is compiled into binary via `include_str!()`
