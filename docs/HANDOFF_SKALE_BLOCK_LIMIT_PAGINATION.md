# Handoff: SKALE Block Range Limit — Needs Pagination or Direct Call

## Status

v1.41.0 still fails on SKALE. Both owner lookup and register idempotency scan Transfer events via `eth_getLogs`, which SKALE limits to 2000 blocks.

```
GET /identity/skale-base/owner/0x52E05C8e...
→ {"error": "Block range limit exceeded. Maximum allowed number of requested blocks is 2000"}

POST /register (same wallet)
→ Created #250 (5th duplicate — #246-#250 all same owner)
```

## The Simplest Fix: balanceOf + tokenOfOwnerByIndex (NO event scanning)

Don't scan events at all. Use view functions:

```rust
// For owner lookup AND register idempotency:
let balance: U256 = identity_contract.balance_of(address).call().await?;

if balance > U256::zero() {
    // ERC-721 Enumerable: tokenOfOwnerByIndex(owner, 0)
    let token_id: U256 = identity_contract
        .token_of_owner_by_index(address, U256::zero())
        .call()
        .await?;

    // Return existing token — don't mint
    return existing_response(token_id);
}
```

These are `eth_call` (view functions), not `eth_getLogs`. No block range limits apply.

## Does tokenOfOwnerByIndex work on SKALE?

Test:
```bash
# balanceOf — confirmed working
cast call 0x8004A169FB4a3325136EB29fA0ceB6D2e539a432 \
  "balanceOf(address)(uint256)" \
  0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15 \
  --rpc-url https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague

# tokenOfOwnerByIndex — test this
cast call 0x8004A169FB4a3325136EB29fA0ceB6D2e539a432 \
  "tokenOfOwnerByIndex(address,uint256)(uint256)" \
  0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15 0 \
  --rpc-url https://mainnet.skalenodes.com/v1/honorable-steel-rasalhague
```

If `tokenOfOwnerByIndex` also fails, then paginate `eth_getLogs` in chunks of 2000 blocks as fallback. But try direct calls first — they're simpler and faster.

## Urgency

Every test creates another duplicate NFT. Currently at 5 (#246-#250). Please test locally before deploying.
