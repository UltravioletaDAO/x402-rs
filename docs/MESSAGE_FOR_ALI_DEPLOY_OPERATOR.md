Hey Ali,

The new Shanghai-compatible factories on SKALE are deployed and all 21 protocol contracts verified on-chain. Our facilitator is updated to v1.40.2 with the new addresses from your `A1igator/sync-abis-data-param` branch.

We need you to deploy a PaymentOperator for us on SKALE Base. We tried doing it ourselves but the new factories reject our calls:

**What we tried:**
1. `deployOperator()` with all-zero conditions -- reverts. The new factory no longer accepts all-zero configs (the old factory did). It needs real condition contracts (EscrowPeriod, RefundRequest, OrCondition, etc.)
2. Individual factory deploys (`escrowPeriodFactory.deploy(604800, bytes32(0))`) -- also reverts on SKALE
3. We read your SDK's `presets.ts` and understand the full `deployMarketplaceOperator()` flow, but the factory ABIs changed in the Shanghai redeploy and we can't match the encoding with cast/web3.py

**What works:**
- `getDeployed()` view calls work on all factories
- `ESCROW()` and `PROTOCOL_FEE_CONFIG()` return correct addresses
- All 21 protocol contracts have bytecode on SKALE

**What we need:**

Could you run `deployMarketplaceOperator()` on SKALE Base (chain 1187947933)?

```typescript
await deployMarketplaceOperator(walletClient, publicClient, {
  chainId: 1187947933,
  feeRecipient: '0x103040545AC5031A11E8C03dd11324C7333a13C7', // our facilitator
  arbiter: '<your standard arbiter address>',
  escrowPeriodSeconds: 604800n, // 7 days
});
```

Just send me the deployed operator address and we'll register it in the facilitator, update both SDKs, and SKALE escrow goes live.

Everything else is ready:
- Facilitator v1.40.2 has the Shanghai CREATE3 addresses
- SKALE Base in ESCROW_NETWORKS
- Legacy tx handling works
- Both SDKs (TS v2.31.1, Python v0.18.0) have SKALE support
- SKALE docs PR #255 is approved and ready

Thanks!
