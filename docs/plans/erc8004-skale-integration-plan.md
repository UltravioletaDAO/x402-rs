# ERC-8004 SKALE Integration - Master Plan

**Date**: 2026-03-21 (updated 2026-03-21)
**Status**: PHASE 1+2 COMPLETE -- awaiting deploy (Phase 3)
**Priority**: High - aligns with x402r/Execution Market timeline
**Dependency**: Ali from x402r delivering Execution Market support by 2026-03-22

---

## Context

Add ERC-8004 (Trustless Agents) support for SKALE Base Mainnet and SKALE Base Sepolia to the x402-rs facilitator. SKALE's gasless model (sFUEL) means all ERC-8004 operations cost zero gas -- ideal for high-frequency Execution Market reputation.

### Current State

| Component | Status |
|-----------|--------|
| SKALE x402 payment support (USDC settlements) | Code complete, NOT deployed to ECS |
| ERC-8004 SKALE Rust code (`src/erc8004/mod.rs`) | COMPLETE (Phase 1, 2026-03-21) |
| Terraform RPC vars for SKALE | COMPLETE (Phase 2, 2026-03-21) |
| ERC-8004 contracts verified on-chain (4/4) | CONFIRMED (bytecode length 262) |
| ERC-8004 EVM implementation (10 mainnets + 8 testnets) | Code ready, deploy pending |
| ERC-8004 Solana implementation (2 networks) | Production |
| x402r/Execution Market integration | Ali delivering 2026-03-22 |

### Contract Addresses (Confirmed)

**SKALE Base Mainnet (Chain ID: 1187947933)**

| Contract | Address |
|----------|---------|
| IdentityRegistry | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` |
| ReputationRegistry | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| ValidationRegistry | Not deployed (TEE spec under review) |

**SKALE Base Sepolia (Chain ID: 324705682)**

| Contract | Address |
|----------|---------|
| IdentityRegistry | `0x8004A818BFB912233c491871b3d84c89A494BD9e` |
| ReputationRegistry | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| ValidationRegistry | Not deployed |

### SKALE-Specific Notes

- **Gasless**: sFUEL is free. No native token funding needed for facilitator wallet.
- **No EIP-1559**: Already handled via `is_eip1559() -> false` in `src/chain/evm.rs:617-618`.
- **Public RPCs** (no API key needed):
  - Mainnet: `https://skale-base.skalenodes.com/v1/base`
  - Testnet: `https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha`

---

## PHASE 1: Rust Code -- ERC-8004 SKALE Support

**Goal**: Add SKALE to the ERC-8004 contract registry so all existing endpoints work for SKALE.
**Agent**: AEGIS (Rust architect) or general-purpose
**Parallelizable**: Yes (independent of Phase 2 and Phase 3)

### Task 1.1: Add SKALE Contract Constants

**File**: `src/erc8004/mod.rs`
**Location**: After line ~152 (after `AVALANCHE_MAINNET_CONTRACTS`)

Add two new constants:

```rust
// SKALE Base Mainnet - Official deployment (CREATE2 deterministic)
// Gasless L3 on Base - zero gas cost for reputation operations
pub const SKALE_BASE_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// SKALE Base Sepolia Testnet - Official testnet deployment (CREATE2 deterministic)
pub const SKALE_BASE_SEPOLIA_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: None,
};
```

### Task 1.2: Update `get_contracts()` and `supported_networks()`

**File**: `src/erc8004/mod.rs`

**In `get_contracts()` match (around line 224)**, add before `_ => None`:
```rust
Network::SkaleBase => Some(SKALE_BASE_MAINNET_CONTRACTS),
Network::SkaleBaseSepolia => Some(SKALE_BASE_SEPOLIA_CONTRACTS),
```

**In `supported_networks()` vec (around line 244)**, add:
```rust
// After Network::Avalanche in mainnets section:
Network::SkaleBase,
// After Network::AvalancheFuji in testnets section:
Network::SkaleBaseSepolia,
```

**In module doc comments (top of file)**, add:
```
//! - SKALE Base Mainnet (gasless L3)    // in EVM Mainnets section
//! - SKALE Base Sepolia                  // in EVM Testnets section
```

### Task 1.3: Update All Tests

**File**: `src/erc8004/mod.rs`

**`test_all_mainnets_use_deterministic_addresses`**: Add `Network::SkaleBase` to `mainnet_networks` vec.

**`test_all_testnets_use_testnet_addresses`**: Add `Network::SkaleBaseSepolia` to `testnet_networks` vec. IMPORTANT: SKALE testnet does NOT have ValidationRegistry. Change the assertion to be conditional:
```rust
// Most testnets have validation registry, but some (like SKALE) don't yet
if network != Network::SkaleBaseSepolia {
    assert!(
        contracts.validation_registry.is_some(),
        "Network {:?} should have validation registry",
        network
    );
}
```

**`test_supported_networks_list`**: Change count from 18 to 20.

**`test_supported_network_names`**: Change count from 18 to 20. Add:
```rust
assert!(names.contains(&"skale-base".to_string()));
assert!(names.contains(&"skale-base-sepolia".to_string()));
```

**Verify**: Run `cargo test -p x402-rs -- erc8004` -- all tests must pass.

---

## PHASE 2: Infrastructure -- Terraform and Deployment Unblock

**Goal**: Fix the SKALE deployment gap so SKALE works in production (payments + ERC-8004).
**Agent**: TERRAFORM (AWS architect)
**Parallelizable**: Yes (independent of Phase 1, must complete before Phase 3)

### Task 2.1: Add SKALE RPC Variables to Terraform

**File**: `terraform/environments/production/main.tf`

Add to the ECS task definition `environment` block (NOT `secrets` -- these are public RPCs):

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

### Task 2.2: Verify Contracts Exist On-Chain

Before deploying, verify the ERC-8004 contracts are actually deployed on SKALE:

```bash
# IdentityRegistry on SKALE mainnet
curl -s "https://skale-base.skalenodes.com/v1/base" \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getCode","params":["0x8004A169FB4a3325136EB29fA0ceB6D2e539a432","latest"],"id":1}' \
  | jq '.result | length'
# Must return > 2 (not "0x" = no contract)

# ReputationRegistry on SKALE mainnet
curl -s "https://skale-base.skalenodes.com/v1/base" \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getCode","params":["0x8004BAa17C55a88189AE136b182e5fdA19dE9b63","latest"],"id":1}' \
  | jq '.result | length'

# IdentityRegistry on SKALE testnet
curl -s "https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha" \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getCode","params":["0x8004A818BFB912233c491871b3d84c89A494BD9e","latest"],"id":1}' \
  | jq '.result | length'

# ReputationRegistry on SKALE testnet
curl -s "https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha" \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_getCode","params":["0x8004B663056A597Dffe9eCcC1965A193B7388713","latest"],"id":1}' \
  | jq '.result | length'
```

If any return `"0x"`, STOP -- the contract is not deployed on that network.

### Task 2.3: Push SKALE Payment Code to Remote

The SKALE x402 payment code exists locally but was never pushed:

```bash
# Verify what's unpushed
git log origin/main..HEAD --oneline | grep -i skale

# Push to remote (after Phase 1 code is committed)
git push origin main
```

This unblocks both SKALE payments AND SKALE ERC-8004 in production.

---

## PHASE 3: Deploy, Verify, and x402r Integration

**Goal**: Deploy to production, verify SKALE ERC-8004 works, prepare for Ali's Execution Market.
**Agent**: FOREMAN (task orchestrator) or manual
**Parallelizable**: No -- depends on Phase 1 + Phase 2 completion
**Timeline**: After Phase 1 + 2 complete; x402r integration after Ali delivers (~2026-03-22)

### Task 3.1: Build and Deploy

```bash
# 1. Check current deployed version
curl -s https://facilitator.ultravioletadao.xyz/version

# 2. Bump version in Cargo.toml (from deployed version, NOT local)
# Edit Cargo.toml

# 3. Terraform apply (if not already done in Phase 2)
cd terraform/environments/production
terraform plan -out=skale-erc8004.tfplan
terraform apply skale-erc8004.tfplan

# 4. Build and push Docker image
./scripts/fast-build.sh <new-version> --push

# 5. Force new ECS deployment
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

### Task 3.2: Production Verification

```bash
# 1. SKALE appears in ERC-8004 supported networks
curl -s https://facilitator.ultravioletadao.xyz/feedback | jq '.supportedNetworks'
# Must include "skale-base" and "skale-base-sepolia"

# 2. SKALE appears in /supported (x402 payments)
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | contains("skale"))'

# 3. Identity total supply on SKALE
curl -s https://facilitator.ultravioletadao.xyz/identity/skale-base/total-supply

# 4. Reputation read on SKALE
curl -s https://facilitator.ultravioletadao.xyz/reputation/skale-base/1

# 5. Test feedback submission on SKALE testnet
curl -X POST https://facilitator.ultravioletadao.xyz/feedback \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "skale-base-sepolia",
    "feedback": {
      "agentId": 1,
      "value": 90,
      "valueDecimals": 0,
      "tag1": "quality",
      "tag2": "integration-test"
    }
  }'

# 6. Register facilitator agent on SKALE testnet
curl -X POST https://facilitator.ultravioletadao.xyz/register \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "skale-base-sepolia",
    "agentUri": "https://facilitator.ultravioletadao.xyz/.well-known/agent.json"
  }'

# 7. Logo accessible
curl -sI https://facilitator.ultravioletadao.xyz/skale.png | head -5
```

### Task 3.3: x402r Execution Market Integration (after Ali delivers ~2026-03-22)

When Ali confirms Execution Market is ready:

1. **Register facilitator on SKALE mainnet Identity Registry**:
```bash
curl -X POST https://facilitator.ultravioletadao.xyz/register \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "skale-base",
    "agentUri": "https://facilitator.ultravioletadao.xyz/.well-known/agent.json"
  }'
```

2. **Test Execution Market round-trip on SKALE testnet**:
   - Ali's system discovers our facilitator via Identity Registry
   - Ali's system routes a task to our facilitator
   - Our facilitator settles payment on SKALE (gasless)
   - Feedback is posted to Reputation Registry on SKALE (gasless)
   - Verify reputation score updates

3. **Coordinate with Ali for mainnet activation**:
   - Confirm contract addresses match on both sides
   - Verify CAIP-2 format: `eip155:1187947933` for SKALE Base
   - Test end-to-end payment+reputation flow
   - Green-light mainnet Execution Market on SKALE

---

## Agent Dispatch Summary

```
PHASE 1 (Rust Code)          PHASE 2 (Infrastructure)
    |                              |
    v                              v
[AEGIS Agent]                [TERRAFORM Agent]
 Task 1.1: Constants          Task 2.1: Terraform RPC vars
 Task 1.2: Match arms         Task 2.2: Verify contracts on-chain
 Task 1.3: Tests              Task 2.3: Push code to remote
    |                              |
    +----------+-------------------+
               |
               v
         PHASE 3 (Deploy + Verify)
               |
               v
         [FOREMAN Agent / Manual]
          Task 3.1: Build + Deploy
          Task 3.2: Production verification
          Task 3.3: x402r integration (after Ali, ~2026-03-22)
```

**Phase 1 and Phase 2 run in parallel.**
**Phase 3 runs after both complete.**

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Contracts not deployed on SKALE | ERC-8004 calls fail | Task 2.2 verifies with `eth_getCode` |
| SKALE RPC unreliable | Timeouts | SKALE provides SLA on public RPCs |
| ValidationRegistry missing | No validation features | Set to `None`; spec-wide TEE review |
| x402r Execution Market delayed | Integration blocked | SKALE ERC-8004 is valuable independently |
| SKALE legacy tx issues | Transaction failures | Already handled by `is_eip1559() -> false` |

---

## Why SKALE for Execution Market

| Operation | Cost on Base | Cost on SKALE |
|-----------|-------------|---------------|
| Register agent | ~$0.02 gas | FREE |
| Submit feedback | ~$0.01 gas | FREE |
| Read reputation | Free (view) | Free (view) |
| USDC settlement | ~$0.02 gas | FREE |

For a high-frequency execution market, SKALE eliminates gas costs entirely.

---

## References

- [ERC-8004 Contracts Repo](https://github.com/erc-8004/erc-8004-contracts)
- [ERC-8004 Specification](https://eips.ethereum.org/EIPS/eip-8004)
- [SKALE Documentation](https://docs.skale.space/)
- [SKALE Integration Plan (x402 payments)](docs/SKALE_INTEGRATION_PLAN.md)
- [ERC-8004 Integration Plan (original)](docs/ERC8004_INTEGRATION.md)
- [ERC-8004 Solana Integration](docs/ERC8004_SOLANA_INTEGRATION.md)
- Implementation: `src/erc8004/mod.rs`, `src/erc8004/types.rs`, `src/erc8004/abi.rs`
