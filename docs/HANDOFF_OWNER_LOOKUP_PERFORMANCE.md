# Handoff: Owner Lookup Takes ~14s on SKALE

## Problem

`GET /identity/skale-base/owner/0x52E05C8e...` takes **~14 seconds**. This causes downstream timeouts in Execution Market (ALB 30s limit, task creation includes identity + escrow + geocoding).

```bash
time curl -s "https://facilitator.ultravioletadao.xyz/identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15"
# real 13.6s
```

## Root Cause

The endpoint iterates `ownerOf(tokenId)` from token 1 to totalSupply (~250 on SKALE). Each call is a separate RPC request. 250 sequential `eth_call`s × ~50ms each ≈ 12-14s.

## Fix Options

**Option A (recommended): Batch RPC / multicall**

Bundle multiple `ownerOf` calls into a single `eth_call` via Multicall3 (`0xcA11bde05977b3631167028862bE2a173976CA11` — deployed on SKALE):

```rust
// 1 RPC call instead of 250:
let calls: Vec<Call3> = (1..=total_supply)
    .map(|id| Call3 {
        target: registry,
        callData: ownerOf(id).encode(),
        allowFailure: true,
    })
    .collect();
let results = multicall3.aggregate3(calls).call().await?;
// Find first result where owner == target_address
```

This reduces 250 RPC calls to 1, bringing latency from ~14s to <1s.

**Option B: Binary search**

If tokens are minted sequentially (token 1 = first ever, token 250 = latest), search from the end:

```rust
// Most recent registrations are at higher IDs
for id in (1..=total_supply).rev() {
    if ownerOf(id) == target_address {
        return id;  // Found on first match
    }
}
```

Still sequential but finds recent registrations faster. Worst case is the same.

**Option C: In-memory cache at facilitator**

After first resolution, cache `(network, owner) → agentId` for 5 minutes. Subsequent lookups are instant. Invalidate on new `/register` calls.

## Impact

EM has a client-side 8s timeout workaround — if owner lookup exceeds 8s, the task is created without `erc8004_agent_id`. This works but means SKALE tasks often lack their agent ID. Fixing the latency eliminates this.

## Benchmark

```bash
# Current (v1.41.1):
time curl -s "https://facilitator.ultravioletadao.xyz/identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15"
# Target: < 2 seconds
```
