# Handoff: GET /identity/{network}/owner/{address} endpoint

## Problem

On chains without ERC-721 Enumerable (like SKALE), `tokenOfOwnerByIndex(address, 0)` reverts. This means callers can verify a wallet owns an NFT via `balanceOf` but **cannot resolve the token ID**.

Currently, `/register` creates a NEW NFT every time it's called — even if the wallet already owns one. This caused wallet `0x52E05C8e...` to accumulate 3 duplicate NFTs on SKALE (#246, #247, #248).

## Two fixes needed

### Fix 1: `GET /identity/{network}/owner/{address}` (new endpoint)

Return the first agent ID owned by a wallet address on a given network.

```
GET /identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15

Response:
{
  "agentId": 246,
  "owner": "0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15",
  "agentUri": "https://execution.market/workers/0x52e05c8e...",
  "network": "skale-base",
  "balance": 3
}
```

Implementation options:
- The facilitator already indexes identity events — query the DB/index for tokens owned by address
- Or use `tokenOfOwnerByIndex` where available, fallback to event log scan on non-Enumerable chains
- Or maintain a `(network, owner) → agentId` mapping table populated at mint time

### Fix 2: `/register` should be idempotent

Before minting, check `balanceOf(recipient)`. If > 0, return the existing agent ID instead of creating a duplicate.

```rust
// Pseudocode in register handler:
let balance = contract.balance_of(recipient).await?;
if balance > 0 {
    // Resolve existing token ID and return it
    let existing_id = resolve_token_by_owner(network, recipient).await?;
    return Ok(RegisterResponse {
        success: true,
        agentId: existing_id,
        idempotent: true,
    });
}
// Otherwise proceed with mint
```

## Impact

Without Fix 1, Execution Market tasks on SKALE show `erc8004_agent_id: null` because we can't resolve the token ID. We stopped re-registering (Fix 2 client-side workaround), but the agent ID is unknown.

Without Fix 2, any client that calls `/register` without checking first creates duplicates.

## Affected chains

Any chain where the ERC-8004 Identity Registry was deployed WITHOUT ERC-721 Enumerable extension. Currently confirmed: **SKALE**. Potentially others.
