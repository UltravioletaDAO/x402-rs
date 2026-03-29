Hey Ali,

We updated our facilitator to v1.40.2 with all the new addresses from your `A1igator/sync-abis-data-param` branch. All 21 protocol contracts verified on-chain on SKALE Base. Everything on our side is ready.

We tried deploying a PaymentOperator on SKALE ourselves and hit a blocker: **none of the factory `deploy()` calls work on SKALE**. View functions work fine, but any state-changing call reverts.

**What we tested:**

| Call | Result |
|------|--------|
| `EscrowPeriodFactory.ESCROW()` | Works -- returns `0xe050bB89...` |
| `EscrowPeriodFactory.getDeployed(604800, bytes32(0))` | Works -- returns `0x0` (not deployed) |
| `EscrowPeriodFactory.deploy(604800, bytes32(0))` via `eth_call` | Returns `0x` (empty, should return address) |
| `EscrowPeriodFactory.deploy(604800, bytes32(0))` via real tx (10M gas) | **Reverts**, consuming all gas |
| `RefundRequestFactory.computeAddress(address)` | Returns `0x` (empty) |
| `RefundRequestFactory.deploy(address)` via `eth_call` | Returns `0x` |
| `PaymentOperatorFactory.deployOperator(config)` | **Reverts** |

Same pattern on every factory: view functions respond correctly, but `deploy()` and `computeAddress()` return empty or revert.

**What we tried (all failed on SKALE):**
1. Your SDK directly (`npx tsx` with `deployMarketplaceOperator()`) -- fails on `computeAddress`
2. `cast send --legacy` with various gas limits (600k, 3M, 5M, 10M)
3. Raw `eth_call` via curl with exact function selectors
4. Python web3.py with manual ABI encoding

**Our theory:** The CREATE2 inside `deploy()` reverts because the child contract init code contains opcodes that SKALE doesn't execute correctly. The factory dispatcher works (view functions), but the child contract creation fails.

**Can you:**
1. Try running `deployMarketplaceOperator()` on SKALE yourself to confirm?
2. Compare factory bytecode on SKALE vs Base mainnet -- are they identical?
3. If you can deploy successfully, send us the operator address with this config:

```typescript
{
  chainId: 1187947933,
  feeRecipient: '0x103040545AC5031A11E8C03dd11324C7333a13C7',
  arbiter: '<your standard arbiter>',
  escrowPeriodSeconds: 604800n,
}
```

Once we have the operator address, everything else is ready to go:
- Facilitator v1.40.2 with Shanghai CREATE3 addresses
- SKALE in ESCROW_NETWORKS
- Both SDKs (TS v2.31.1, Python v0.18.0) with SKALE support
- SKALE docs PR #255 approved

Thanks!
