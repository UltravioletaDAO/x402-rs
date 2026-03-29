Hey Ali,

We cloned your SDK (`A1igator/sync-abis-data-param` branch), installed it, and ran `deployMarketplaceOperator()` on SKALE Base. It fails on the RefundRequestFactory:

```
ContractFunctionExecutionError: The contract function "computeAddress" returned no data ("0x").

Contract Call:
  address:   0x7996b1E7B5B28AF85093dcE3AE73b128133D3715
  function:  computeAddress(address arbiter)
  args:      (0x103040545AC5031A11E8C03dd11324C7333a13C7)
```

The RefundRequestFactory at `0x7996b1E7B5B28AF85093dcE3AE73b128133D3715` on SKALE doesn't respond to `computeAddress(address)` -- returns empty bytes. The SDK's ABI expects this function to exist.

This might mean the contract deployed on SKALE is a different version than what's on other chains, or the Shanghai recompile changed the function signatures.

We also tested all the factories manually:

- `ESCROW()` on EscrowPeriodFactory -- returns `0xe050bB89...` correctly
- `getDeployed(604800, bytes32(0))` -- returns `0x0` (not deployed, correct)
- `deploy(604800, bytes32(0))` via `eth_call` -- returns `0x` (empty, should return address)
- `deploy(604800, bytes32(0))` via real tx (10M gas) -- reverts, consuming all gas

Same pattern on RefundRequestFactory:
- `deploy(address)` via `eth_call` -- returns `0x`
- View functions work fine

The factories exist and respond to view calls, but all `deploy()` calls fail. This means the CREATE2 inside deploy() reverts -- likely the child contract bytecode has opcodes SKALE doesn't support, or there's an initialization error specific to SKALE.

Can you:
1. Try running `deployMarketplaceOperator()` yourself on SKALE to see if you get the same error?
2. Compare the EscrowPeriodFactory bytecode on SKALE vs Base mainnet?
3. Check if the child contract init code uses any Cancun-specific opcodes?

We tried everything from our side: your SDK directly (`npx tsx`), cast, web3.py, raw RPC calls. All deploy() calls revert on SKALE.

Thanks!
