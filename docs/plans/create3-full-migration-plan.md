# CREATE3 Full Migration Plan — From Legacy to Unified Addresses

**Date**: 2026-03-22
**Status**: PLANNING (blocked on Ali fixing SKALE Cancun EVM compat)
**Current State**: v1.40.1 hybrid model (legacy for existing, CREATE3 for SKALE only)
**Goal**: All networks on CREATE3 unified addresses

---

## Context

Ali redeployed all x402r infrastructure via CREATE3 (same address on every chain). Our facilitator currently uses a hybrid model:
- **Existing networks** (Base, Ethereum, Polygon, etc.): legacy per-chain addresses
- **New networks** (SKALE): CREATE3 unified addresses

To fully migrate, we need to:
1. Deploy NEW PaymentOperators on each network against the CREATE3 infrastructure
2. Update the facilitator to use CREATE3 addresses for all networks
3. Update both SDKs (Python + TypeScript) with new addresses
4. Update the Execution Market to use the new operators
5. Retire old operators

---

## Blockers

| Blocker | Owner | Status |
|---------|-------|--------|
| SKALE: Cancun opcodes (TSTORE/TLOAD) in PaymentOperator | Ali | WAITING - needs recompile with `evm_version = "shanghai"` |
| Confirm no active escrows on old contracts | Ali | WAITING |

---

## Phase 1: Deploy New PaymentOperators (Per-Chain)

**Owner**: Us (facilitator)
**Dependency**: Ali confirms no active escrows on old contracts
**Tool**: `scripts/deploy_operator.py` or x402r SDK `deployMarketplaceOperator()`

For each network, deploy a new PaymentOperator via the CREATE3 factory (`0xdc41F932...`). The operator config must match Fase 5 (or whatever the current standard is).

The x402r SDK's `deployMarketplaceOperator()` handles the full flow:
1. Deploys EscrowPeriod, RefundRequest, StaticAddressCondition, RefundRequestEvidence
2. Deploys FeeCalculator (if fee > 0)
3. Deploys PaymentOperator with correct condition references
4. All in a single Multicall3 batch transaction

**Required parameters** (per `MarketplaceOperatorOptions`):
```typescript
{
  chainId: number,               // e.g. 8453 for Base
  feeRecipient: Address,         // facilitator wallet
  arbiter: Address,              // dispute resolution address (ask Ali)
  escrowPeriodSeconds: bigint,   // e.g. 604800n (7 days)
  operatorFeeBps?: bigint,       // e.g. 1300n for 13%
  freezeDurationSeconds?: bigint, // optional
}
```

### Networks to deploy on:

| Network | Chain ID | Factory | Facilitator Wallet |
|---------|----------|---------|-------------------|
| Base Sepolia | 84532 | `0xdc41F932...` | `0x34033041...` |
| Ethereum Sepolia | 11155111 | `0xdc41F932...` | `0x34033041...` |
| Base | 8453 | `0xdc41F932...` | `0x103040545...` |
| Ethereum | 1 | `0xdc41F932...` | `0x103040545...` |
| Polygon | 137 | `0xdc41F932...` | `0x103040545...` |
| Arbitrum | 42161 | `0xdc41F932...` | `0x103040545...` |
| Celo | 42220 | `0xdc41F932...` | `0x103040545...` |
| Monad | 143 | `0xdc41F932...` | `0x103040545...` |
| Avalanche | 43114 | `0xdc41F932...` | `0x103040545...` |
| Optimism | 10 | `0xdc41F932...` | `0x103040545...` |
| SKALE Base | 1187947933 | `0xdc41F932...` | `0x103040545...` |

**Note**: SKALE requires Ali to recompile with `evm_version = "shanghai"` first.

### Deployment method options:

**Option A: Use x402r SDK (recommended)**
```bash
npx ts-node deploy-all-operators.ts
```
Write a TypeScript script that calls `deployMarketplaceOperator()` for each chain. This handles all prerequisite contracts automatically.

**Option B: Use our deploy_operator.py**
Update the script to support the new CREATE3 factory address and the full OperatorConfig with non-zero conditions. More work than Option A.

**Option C: Ask Ali to deploy for us**
Ali's SDK handles everything. He can deploy operators on all chains with one script.

---

## Phase 2: Update Facilitator (addresses.rs)

**Owner**: Us
**Dependency**: Phase 1 (need new operator addresses)

Replace the hybrid model in `src/payment_operator/addresses.rs`:

```rust
// Change from:
Network::Base => Some(Self {
    escrow: base_mainnet::ESCROW,           // legacy
    token_collector: base_mainnet::TOKEN_COLLECTOR,  // legacy
    payment_operators: vec![old_operator],
    ...
}),

// To:
Network::Base => Some(Self {
    escrow: create3::ESCROW,                // CREATE3 unified
    token_collector: create3::TOKEN_COLLECTOR,       // CREATE3 unified
    payment_operators: vec![new_operator],   // deployed in Phase 1
    ...
}),
```

All networks use `create3::` constants. Per-chain modules can be deleted entirely.

---

## Phase 3: Update SDKs

**Owner**: SDK agents (Python + TypeScript)

### TypeScript SDK (`uvd-x402-sdk-typescript`)

Update escrow addresses to CREATE3 unified:
```typescript
// Before (per-chain):
const BASE_ESCROW = "0xb9488351...";
const BASE_TOKEN_COLLECTOR = "0x48ADf6E3...";

// After (unified):
const ESCROW = "0xe050bB89eD43BB02d71343063824614A7fb80B77";
const TOKEN_COLLECTOR = "0xcE66Ab399EDA513BD12760b6427C87D6602344a7";
```

Update operator addresses per network with the new ones from Phase 1.

### Python SDK (`uvd-x402-sdk-python`)

Same changes as TypeScript SDK.

---

## Phase 4: Update Execution Market

**Owner**: Execution Market agent

The Execution Market needs to:

1. **Update its PaymentOperator reference** per network to the new Phase 1 operators
2. **Update escrow/tokenCollector addresses** in payment requirements to CREATE3 unified
3. **Test end-to-end** on each network:
   - Create escrow payment with new operator
   - Verify escrow state
   - Release/charge payment
   - Verify funds settled correctly

The `requirements.extra` in escrow requests must send:
```json
{
  "escrowAddress": "0xe050bB89eD43BB02d71343063824614A7fb80B77",
  "operatorAddress": "<new-operator-from-phase-1>",
  "tokenCollector": "0xcE66Ab399EDA513BD12760b6427C87D6602344a7"
}
```

---

## Phase 5: Retire Old Operators

**Owner**: Us + Ali
**Dependency**: Phase 4 complete, no active escrows on old operators

1. Verify all pending escrows on old operators are settled (released/charged/refunded)
2. Remove old operator addresses from `addresses.rs`
3. Remove old per-chain address modules
4. Update tests
5. Deploy clean version

---

## Coordination Checklist

| Step | Who | Action | Status |
|------|-----|--------|--------|
| 1 | Ali | Fix SKALE EVM compat (ReentrancyGuard) | WAITING |
| 2 | Ali | Confirm no active escrows on old contracts | WAITING |
| 3 | Ali | Confirm arbiter address for operator deployment | WAITING |
| 4 | Us | Deploy new operators on all networks (Phase 1) | BLOCKED on 2+3 |
| 5 | Us | Update facilitator addresses.rs (Phase 2) | BLOCKED on 4 |
| 6 | SDK agents | Update Python + TypeScript SDKs (Phase 3) | BLOCKED on 4 |
| 7 | EM agent | Update Execution Market (Phase 4) | BLOCKED on 5+6 |
| 8 | Us + Ali | Retire old operators (Phase 5) | BLOCKED on 7 |

---

## CREATE3 Unified Addresses Reference

All networks will use these addresses after full migration:

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0xe050bB89eD43BB02d71343063824614A7fb80B77` |
| TokenCollector | `0xcE66Ab399EDA513BD12760b6427C87D6602344a7` |
| ProtocolFeeConfig | `0x7e868A42a458fa2443b6259419aA6A8a161E08c8` |
| PaymentOperatorFactory | `0xdc41F932dF2d22346F218E4f5650694c650ab863` |
| RefundRequestFactory | `0x9cD87Bb58553Ef5ad90Ed6260EBdB906a50D6b83` |
| RefundRequestEvidenceFactory | `0x3769Be76BBEa31345A2B2d84EF90683E9A377e00` |
| UsdcTvlLimit | `0x0F1F26719219CfAdC8a1C80D2216098A0534a091` |
| ArbiterRegistry | `0x1c2d7d5978d46a943FA98aC9a649519C1424FB3e` |
| ReceiverRefundCollector | `0xE5500a38BE45a6C598420fbd7867ac85EC451A07` |
| Condition: Payer | `0x33F5F1154A02d0839266EFd23Fd3b85a3505bB4B` |
| Condition: Receiver | `0xF41974A853940Ff4c18d46B6565f973c1180E171` |
| Condition: AlwaysTrue | `0xb295df7E7f786fd84D614AB26b1f2e86026C3483` |

PaymentOperator addresses will be per-chain (deployed via factory in Phase 1).
