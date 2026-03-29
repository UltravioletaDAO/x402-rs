# Handoff: Owner Lookup + Idempotent Register — SKALE Block Limit

## Current State

Both fixes from the previous handoff need adjustments for SKALE:

1. `GET /identity/skale-base/owner/0x52E05C8e...` returns:
```json
{"error": "Failed to scan events: Block range limit exceeded. Maximum allowed number of requested blocks is 2000"}
```

2. `POST /register` still creates duplicates — #249 just minted (wallet now has 4 NFTs: #246, #247, #248, #249).

## Root Cause

SKALE limits `eth_getLogs` to 2000 blocks per request. The event scan approach doesn't work without pagination.

## Fix Options for Owner Lookup

**Option A (recommended): Use balanceOf + tokenOfOwnerByIndex**

Skip event scanning entirely. Use the on-chain view functions:

```rust
// 1. balanceOf(owner) — works on SKALE
let balance = contract.balance_of(owner).call().await?;
if balance == 0 { return NotFound; }

// 2. tokenOfOwnerByIndex(owner, 0) — returns first token ID
let token_id = contract.token_of_owner_by_index(owner, 0).call().await?;
return Ok(IdentityResponse { agentId: token_id, ... });
```

Note: `tokenOfOwnerByIndex` DOES work when called from the facilitator's RPC setup. It only failed from EM's RPC because EM uses a different SKALE endpoint (`skale-base.skalenodes.com/v1/base`). The facilitator's RPC may use a different endpoint or configuration that supports Enumerable.

If `tokenOfOwnerByIndex` also fails on the facilitator's RPC, fall back to:

**Option B: Paginated event scan**

Split the log query into chunks of 2000 blocks:

```rust
let latest = provider.get_block_number().await?;
let mut from_block = 0;
while from_block <= latest {
    let to_block = min(from_block + 1999, latest);
    let logs = provider.get_logs(&filter.from_block(from_block).to_block(to_block)).await?;
    // Check Transfer events TO the owner address
    for log in logs {
        if log.topics[2] == owner_topic {
            return token_id from log.topics[3];
        }
    }
    from_block = to_block + 1;
}
```

**Option C: Cache at mint time**

In the `/register` handler, after minting + transferring, store `(network, owner) → agentId` in a local DB/map. The owner lookup reads from this cache. Simplest but only works for tokens minted by this facilitator.

## Fix for Idempotent Register

The `/register` handler must check BEFORE minting:

```rust
// Before minting:
let balance = contract.balance_of(recipient).call().await?;
if balance > 0 {
    let existing_id = contract.token_of_owner_by_index(recipient, 0).call().await?;
    return Ok(RegisterResponse {
        success: true,
        agent_id: existing_id.to_string(),
        idempotent: true,  // signal no new mint
        network: network,
    });
}
// Proceed with mint only if balance == 0
```

## Test

```bash
# After fix — owner lookup should return #246 (first NFT)
curl https://facilitator.ultravioletadao.xyz/identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15

# After fix — register should return existing, not create #250
curl -X POST https://facilitator.ultravioletadao.xyz/register \
  -H "Content-Type: application/json" \
  -d '{"x402Version":1,"network":"skale-base","agentUri":"https://test","recipient":"0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15"}'
# Expected: {"success":true,"agentId":"246","idempotent":true,...}
```

## Cleanup

After fixing, the 3 duplicate NFTs (#247, #248, #249) should ideally be burned. But that's low priority — they don't cause harm, just clutter.
