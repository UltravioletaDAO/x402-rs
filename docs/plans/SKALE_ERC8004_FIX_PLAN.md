# Master Plan: Fix ERC-8004 Write Operations on Legacy Gas Chains

> **Created**: 2026-03-29
> **Priority**: P0 — Blocks Execution Market identity registration on SKALE
> **Estimated LOC changed**: ~120 lines across 2 files

---

## Full Audit: All 20 ERC-8004 Networks

Cross-reference of all ERC-8004 supported networks against gas model configuration:

### EVM Mainnets (10 chains)

| # | Network | Chain ID | EIP-1559? | ERC-8004 Writes? | Status |
|---|---------|----------|-----------|-----------------|--------|
| 1 | Ethereum | 1 | YES | OK | No issue — Alloy auto-fills EIP-1559 correctly |
| 2 | Base | 8453 | YES | OK | No issue — confirmed working in production |
| 3 | Polygon | 137 | YES | OK | No issue |
| 4 | Arbitrum | 42161 | YES | OK | No issue |
| 5 | Optimism | 10 | YES | OK | No issue |
| 6 | Celo | 42220 | YES | OK | No issue |
| 7 | BSC | 56 | YES | OK | No issue (EIP-1559 since BEP-95) |
| 8 | Monad | 143 | YES | OK | No issue |
| 9 | Avalanche | 43114 | YES | OK | No issue |
| 10 | **SKALE Base** | **1187947933** | **NO** | **BROKEN** | Alloy sends type-2 TX, SKALE rejects `INVALID_PARAMS` |

### EVM Testnets (8 chains)

| # | Network | Chain ID | EIP-1559? | ERC-8004 Writes? | Status |
|---|---------|----------|-----------|-----------------|--------|
| 1 | Ethereum Sepolia | 11155111 | YES | OK | No issue |
| 2 | Base Sepolia | 84532 | YES | OK | No issue |
| 3 | Polygon Amoy | 80002 | YES | OK | No issue |
| 4 | Arbitrum Sepolia | 421614 | YES | OK | No issue |
| 5 | Optimism Sepolia | 11155420 | YES | OK | No issue |
| 6 | Celo Sepolia | 44787 | YES | OK | No issue |
| 7 | Avalanche Fuji | 43113 | YES | OK | No issue |
| 8 | **SKALE Base Sepolia** | **324705682** | **NO** | **BROKEN** | Same bug as mainnet |

### Solana (2 networks) — NOT affected

| # | Network | ERC-8004 Writes? | Status |
|---|---------|-----------------|--------|
| 1 | Solana Mainnet | OK | Separate code path (`src/erc8004/solana.rs`) |
| 2 | Solana Devnet | OK | Separate code path |

### Other Legacy Chains (NOT in ERC-8004) — NOT currently affected

| Network | EIP-1559? | ERC-8004? | Risk |
|---------|-----------|-----------|------|
| XDC | NO | Not supported | No issue now, but would break if added |
| XRPL_EVM | NO | Not supported | No issue now, but would break if added |

### Audit Conclusion

- **2 chains currently broken**: SKALE Base (mainnet) + SKALE Base Sepolia (testnet)
- **18 chains working fine**: All support EIP-1559, Alloy auto-detects correctly
- **Future risk**: Any legacy chain added to ERC-8004 would hit the same bug
- **Fix must be generic**: Check `is_eip1559()` per-network, not SKALE-specific hardcode

---

## Root Cause Analysis

### The Bug

All ERC-8004 write operations (`/register`, `/feedback`, `/feedback/revoke`, `/feedback/response`) fail on SKALE with:

```
INVALID_PARAMS: Invalid method parameters (invalid name and/or type) recognised
```

### Why It Happens

There are **two parallel transaction-sending paths** in the facilitator:

| Path | Used By | Gas Handling | SKALE Works? |
|------|---------|-------------|--------------|
| `chain/evm.rs::send_transaction()` | Payment settlements (`POST /settle`) | Checks `self.eip1559` flag → sets `gasPrice` for legacy chains | **YES** |
| Alloy contract `.send().await` | ERC-8004 operations (`POST /register`, etc.) | Delegates to Alloy's `GasFiller` → auto-detects gas model | **NO** |

**Payment settlements work** because `send_transaction()` (line 365-460 in `evm.rs`) explicitly checks `self.eip1559` and calls `get_gas_price()` + `set_gas_price()` for legacy networks like SKALE.

**ERC-8004 operations fail** because the handlers in `handlers.rs` create Alloy contract instances directly:

```rust
let identity_registry = IIdentityRegistry::new(address, provider.inner().clone());
let call = identity_registry.register_1(agent_uri);
call.send().await  // ← Goes through GasFiller, which auto-detects gas model WRONG for SKALE
```

Alloy's `GasFiller` either:
1. Calls `eth_feeHistory` and gets a partial/unexpected SKALE response, then builds EIP-1559 (type 2) tx
2. Or defaults to EIP-1559 and SKALE rejects the `maxFeePerGas`/`maxPriorityFeePerGas` fields

### Affected Endpoints

1. `POST /register` — `handlers.rs:~3717` (`identity_registry.register_X().send()`)
2. `POST /register` transfer step — `handlers.rs:~3880` (`identity_registry.safeTransferFrom().send()`)
3. `POST /feedback` — `handlers.rs:~2291` (`reputation_registry.giveFeedback().send()`)
4. `POST /feedback/revoke` — `handlers.rs:~2504` (`reputation_registry.revokeFeedback().send()`)
5. `POST /feedback/response` — `handlers.rs:~2717` (`reputation_registry.respondToFeedback().send()`)

### Key Code Locations

- `src/chain/evm.rs:205-218` — `EvmProvider` struct (has private `eip1559` and `inner` fields)
- `src/chain/evm.rs:288-300` — `MetaEvmProvider` trait (exposes `inner()` and `chain()` but NOT `eip1559`)
- `src/chain/evm.rs:365-460` — `send_transaction()` with correct gas handling
- `src/chain/evm.rs:590-634` — `is_eip1559` per-network matrix
- `src/handlers.rs:3658-3730` — ERC-8004 register endpoint (uses `.send()` directly)

---

## Phase 1: Expose Gas Model Info from EvmProvider

**Goal**: Make the `eip1559` flag accessible to handlers without breaking encapsulation.

### Tarea 1.1: Add `is_eip1559()` getter to `MetaEvmProvider` trait

**File**: `src/chain/evm.rs`

Add to the `MetaEvmProvider` trait (around line 288):

```rust
/// Whether the network supports EIP-1559 gas pricing.
fn is_eip1559(&self) -> bool;
```

And implement it on `EvmProvider` (around line 320):

```rust
fn is_eip1559(&self) -> bool {
    self.eip1559
}
```

### Tarea 1.2: Add `send_contract_tx()` helper method to `EvmProvider`

**File**: `src/chain/evm.rs`

Create a new public method on `EvmProvider` that sends an arbitrary `TransactionRequest` with correct gas handling (reusing the same logic from `send_transaction`). This avoids duplicating the gas pricing code:

```rust
/// Send a pre-built TransactionRequest with correct gas pricing for this network.
/// Used by ERC-8004 handlers that build TX via Alloy contract bindings.
pub async fn send_raw_tx(&self, mut txr: TransactionRequest) -> Result<TransactionReceipt, FacilitatorLocalError> {
    // Set from address
    txr.set_from(self.next_signer_address());

    // Apply same gas logic as send_transaction()
    if !self.eip1559 {
        let gas: u128 = self.inner
            .get_gas_price()
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
        txr.set_gas_price(gas);
    }

    // Send and wait for receipt
    let pending_tx = self.inner.send_transaction(txr).await
        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
    let receipt = pending_tx.get_receipt().await
        .map_err(|e| FacilitatorLocalError::ContractCall(format!("{e:?}")))?;
    Ok(receipt)
}
```

**Alternatively** (simpler approach): Instead of a new method, the handlers can just set gas price on the Alloy contract call before `.send()`:

```rust
// Before (broken on SKALE):
let call = identity_registry.register_1(agent_uri);
call.send().await

// After (works on all chains):
let call = identity_registry.register_1(agent_uri);
let call = if !provider.is_eip1559() {
    let gas_price = provider.inner().get_gas_price().await?;
    call.gas_price(gas_price)
} else {
    call
};
call.send().await
```

**Decision**: The simpler "set gas_price on call" approach is recommended for Phase 1. It's less invasive and uses Alloy's existing API. The `send_raw_tx` helper is a Phase 2 optimization if needed.

---

## Phase 2: Fix All ERC-8004 Write Endpoints

**Goal**: Apply the gas pricing fix to all 5 affected code paths.

### Tarea 2.1: Create a helper function for gas-aware contract calls

**File**: `src/handlers.rs` (or `src/erc8004/mod.rs` if preferred)

To avoid repeating the gas logic 5 times, create a small helper:

```rust
use alloy::contract::CallBuilder;

/// Wraps an Alloy contract call with correct gas pricing for the target network.
/// On legacy chains (e.g., SKALE), explicitly sets gasPrice to prevent
/// Alloy's GasFiller from using EIP-1559 fields.
async fn send_erc8004_call<C>(
    call: C,
    provider: &EvmProvider,
) -> Result<PendingTransactionBuilder, Box<dyn std::error::Error>>
where
    C: /* CallBuilder trait bound - exact type depends on Alloy version */
{
    if !provider.is_eip1559() {
        let gas_price = provider.inner().get_gas_price().await?;
        Ok(call.gas_price(gas_price).send().await?)
    } else {
        Ok(call.send().await?)
    }
}
```

> **Nota**: La signatura exacta depende de la versión de Alloy y los trait bounds de `CallBuilder`. Verificar los tipos con `cargo check` en WSL.

### Tarea 2.2: Fix `POST /register` — registration call

**File**: `src/handlers.rs` (around line 3716-3729)

**Before** (current code):
```rust
let register_result = if has_metadata {
    let call = identity_registry.register_0(agent_uri, metadata_entries);
    call.send().await
} else if !request.agent_uri.is_empty() {
    let call = identity_registry.register_1(agent_uri);
    call.send().await
} else {
    let call = identity_registry.register_2();
    call.send().await
};
```

**After**: Each branch needs gas pricing applied before `.send()`. Either use the helper from 2.1 or inline:

```rust
let register_result = if has_metadata {
    let call = identity_registry.register_0(agent_uri, metadata_entries);
    if !provider.is_eip1559() {
        let gp = provider.inner().get_gas_price().await.map_err(|e| /* ... */)?;
        call.gas_price(gp).send().await
    } else {
        call.send().await
    }
} else if !request.agent_uri.is_empty() {
    let call = identity_registry.register_1(agent_uri);
    if !provider.is_eip1559() {
        let gp = provider.inner().get_gas_price().await.map_err(|e| /* ... */)?;
        call.gas_price(gp).send().await
    } else {
        call.send().await
    }
} else {
    let call = identity_registry.register_2();
    if !provider.is_eip1559() {
        let gp = provider.inner().get_gas_price().await.map_err(|e| /* ... */)?;
        call.gas_price(gp).send().await
    } else {
        call.send().await
    }
};
```

### Tarea 2.3: Fix `POST /register` — safeTransferFrom call

**File**: `src/handlers.rs` (around line 3875-3880)

Same pattern — the `safeTransferFrom` call after registration also bypasses gas handling:

```rust
// Before:
match transfer_call.send().await {

// After:
let transfer_result = if !provider.is_eip1559() {
    let gp = provider.inner().get_gas_price().await.map_err(|e| /* ... */)?;
    transfer_call.gas_price(gp).send().await
} else {
    transfer_call.send().await
};
match transfer_result {
```

### Tarea 2.4: Fix `POST /feedback` — giveFeedback call

**File**: `src/handlers.rs` (around line 2285-2291)

```rust
// Before:
match call.send().await {

// After: apply same gas_price pattern
```

### Tarea 2.5: Fix `POST /feedback/revoke` — revokeFeedback call

**File**: `src/handlers.rs` (around line 2499-2504)

Same pattern.

### Tarea 2.6: Fix `POST /feedback/response` — respondToFeedback call

**File**: `src/handlers.rs` (around line 2712-2717)

Same pattern.

---

## Phase 3: Verify sFUEL Balance

**Goal**: Ensure the facilitator wallet has sFUEL on SKALE (required even though gas is "free").

### Tarea 3.1: Check facilitator sFUEL balance

```bash
cast balance 0x103040545AC5031A11E8C03dd11324C7333a13C7 \
  --rpc-url https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague
```

If balance is 0, get sFUEL from https://sfuel.skale.network/ for the facilitator wallet.

### Tarea 3.2: Verify SKALE RPC URL is correct

Check that the RPC URL configured in AWS Secrets Manager (or env var `RPC_URL_SKALE_BASE`) points to the correct SKALE chain endpoint. The chain name for SKALE Base (chain ID 1187947933) should be:

```
https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague
```

Verify chain ID:
```bash
cast chain-id --rpc-url https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague
# Expected: 1187947933
```

---

## Phase 4: Build, Deploy, Test

### Tarea 4.1: Compile in WSL

```bash
cd ~/x402-rs-build  # or wherever the WSL copy lives
# Sync changes from Windows
rsync -av /mnt/z/ultravioleta/dao/x402-rs/ ~/x402-rs-build/ --exclude target --exclude .unused
cargo build --release 2>&1 | head -50
```

Verify no compilation errors. Pay attention to Alloy trait bounds — the `.gas_price()` method on contract calls may need specific imports.

### Tarea 4.2: Build Docker image

```bash
./scripts/fast-build.sh v1.XX.X --push
```

(Bump version from current deployed version)

### Tarea 4.3: Deploy to ECS

```bash
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

### Tarea 4.4: Test registration on SKALE

```bash
# Wait for deployment (~90s)
sleep 90

# Test health
curl -s https://facilitator.ultravioletadao.xyz/health

# Test SKALE registration
curl -X POST https://facilitator.ultravioletadao.xyz/register \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "skale-base",
    "agentUri": "https://execution.market/agents/test-skale-fix",
    "recipient": "0x52e05c8e45a32eee169639f6d2ca40f8887b5a15"
  }'
```

**Expected**: `{"success": true, "agent_id": <number>, ...}`

### Tarea 4.5: Test feedback on SKALE

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/feedback \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "skale-base",
    "agent_id": <agent_id_from_4.4>,
    "value": 100,
    "value_decimals": 0,
    "endpoint": "test",
    "feedback_uri": "ipfs://test"
  }'
```

### Tarea 4.6: Verify on Base (regression test)

```bash
# Ensure Base (EIP-1559) still works
curl -X POST https://facilitator.ultravioletadao.xyz/register \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "base-mainnet",
    "agentUri": "https://test-regression",
    "recipient": "0x52e05c8e45a32eee169639f6d2ca40f8887b5a15"
  }'
```

---

## Phase 5: Notify Execution Market

### Tarea 5.1: Send handoff back to Execution Market

Create `HANDOFF_SKALE_REGISTRATION_FIXED.md` with:
- Confirmation that SKALE registration works
- The agent ID from the test registration
- Any caveats (sFUEL requirements, etc.)

### Tarea 5.2: IRC notification

Notify on `#execution-market-facilitator` that SKALE ERC-8004 writes are fixed.

---

## Summary

| Phase | Tareas | Files Modified | Risk |
|-------|--------|---------------|------|
| 1 - Expose gas model | 2 | `src/chain/evm.rs` | Low — adds getter only |
| 2 - Fix all endpoints | 6 | `src/handlers.rs` | Medium — touches 5 TX paths |
| 3 - Verify sFUEL | 2 | None (infra check) | None |
| 4 - Build/Deploy/Test | 6 | None (ops) | Low — standard deploy |
| 5 - Notify | 2 | Docs only | None |

**Total**: 18 tareas across 5 fases.

**Key insight**: The same `GasFiller` auto-detection bug would affect ANY future legacy-only chain added to the facilitator. The fix should be generic (checking `is_eip1559()`) rather than SKALE-specific.

---

## ERC-8004 Contracts Analysis (from upstream repo)

### Contract Architecture

The ERC-8004 system uses three UUPS-upgradeable contracts with deterministic CREATE2 addresses (all start with `0x8004`):

- **IdentityRegistryUpgradeable** — ERC-721 NFT for agent identities. `register()` is permissionless.
- **ReputationRegistryUpgradeable** — Feedback/reputation tracking. References IdentityRegistry for authorization.
- **ValidationRegistryUpgradeable** — Third-party attestations (not deployed on most chains yet).

### Solidity Compilation

- **Pragma**: `^0.8.20`
- **Compiler**: 0.8.24
- **EVM Version**: `shanghai` (uses `PUSH0` opcode)
- **Optimizer**: enabled, 200 runs, `viaIR: true`
- **Dependencies**: OpenZeppelin `^5.4.0`

### SKALE Compatibility Notes from Contract Code

1. **PUSH0 (EIP-3855)**: Contracts use shanghai EVM. SKALE Base supports this (contracts ARE deployed and working for reads).
2. **No gas-dependent logic**: No `gasleft()`, no `msg.value`, no gas stipend assumptions.
3. **No TSTORE/TLOAD**: No transient storage usage.
4. **No COINBASE**: No `block.coinbase` usage.
5. **`_safeMint`**: Registration uses `_safeMint` which calls `onERC721Received()` on contract recipients. Works fine for EOA callers.
6. **EIP-712 in `setAgentWallet`**: Uses chainId in domain. SKALE's chain ID (1187947933) is supported.

**Conclusion**: The contracts themselves are fully SKALE-compatible. The bug is 100% in the facilitator's transaction submission, not in the smart contracts.
