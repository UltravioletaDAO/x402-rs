# Handoff: Debug release() revert on SKALE Base

## Problem

`release()` reverts on SKALE Base operators. `authorize()` works fine on the same operator. Ali confirmed TSTORE/TLOAD is NOT the issue -- the Shanghai build uses SSTORE/SLOAD via `compat/solady-nontransient/`. The revert is from calldata encoding.

## What works

- `authorize(PaymentInfo, uint256 amount, address tokenCollector, bytes collectorData)` -- succeeds, funds go into escrow
- Lock TX example: `0xf49b66b4607bb5cfd46e37ff8b4eaf57c01467e7ab1f9f51d139d1449547a93a` on SKALE Base

## What fails

- `release(PaymentInfo, uint256 amount, bytes data)` -- reverts with no error message
- `refundInEscrow(PaymentInfo, uint120 amount, bytes data)` -- same

## Key files

- **Our facilitator escrow code**: `src/payment_operator/operator.rs` (lines 665-744)
- **Our ABI**: `abi/PaymentOperator.json` (extracted from Ali's SDK `generated.ts`)
- **Ali's contracts**: https://github.com/BackTrackCo/x402r-contracts
  - `src/operator/payment/PaymentOperator.sol` (release at line 293, refundInEscrow at line 342)
  - `foundry.toml` has `[profile.shanghai]` with solady remap
- **Ali's SDK**: https://github.com/BackTrackCo/x402r-sdk branch `A1igator/sync-abis-data-param`
  - `packages/core/src/abis/generated.ts` -- the ABI we extracted
  - `packages/core/src/deploy/presets.ts` -- how operators are deployed

## On-chain state (SKALE Base, chain 1187947933)

```
Operator:        0x28c23AE8f55aDe5Ea10a5353FC40418D0c1B3d33
Escrow:          0xBC151792f80C0EB1973d56b0235e6bee2A60e245
TokenCollector:  0x9A12A116a44636F55c9e135189A1321Abcfe2f30
USDC.e:          0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20
RPC:             https://skale-base.skalenodes.com/v1/base
```

## PaymentInfo from the lock TX

```
operator:            0x28c23AE8f55aDe5Ea10a5353FC40418D0c1B3d33
payer:               0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15
receiver:            0xe4dc963c56979E0260fc146b87eE24F18220e545
token:               0x85889c8c714505E0c94b30fcfcF64fE3Ac8FCb20
maxAmount:           50000 (0.05 USDC)
preApprovalExpiry:   1774656498
authorizationExpiry: 1774660098
refundExpiry:        1774739298
minFeeBps:           0
maxFeeBps:           1800
feeReceiver:         0x28c23AE8f55aDe5Ea10a5353FC40418D0c1B3d33
salt:                0xe34f58cf9f7f29f8340c55f05e54f35b1e41bdaae915bf3714ed442e9a5e6fad
```

## What we tried

1. `cast call` with correct selector `0xc602dd4a` and all PaymentInfo fields + amount 50000 + empty bytes `0x` -- reverts
2. Rust code using Alloy `sol!` macro with `releaseCall { paymentInfo, amount, data: Bytes::new() }` -- reverts
3. Ali's SDK (`deployMarketplaceOperator`) -- worked for deploy but we haven't tried calling release from his SDK directly

## What to investigate

1. **PaymentInfo encoding**: Does Alloy's `sol!` macro encode the PaymentInfo tuple identically to how the contract expects it? Specifically the packed types: `uint120 maxAmount`, `uint48` timestamps, `uint16` fee bps. Alloy uses `Uint<120, 2>` for uint120 -- verify this pads correctly.

2. **Escrow hash mismatch**: When we queried `paymentState(hash)` on the escrow, it returned 0 (no payment found). This suggests the hash we compute differs from what the escrow stored during authorize. The escrow uses `getHash(PaymentInfo)` -- maybe our PaymentInfo encoding doesn't match.

3. **The bytes data parameter**: Ali's contract comment says use `""` or `"0x"` for no data. Verify that `Bytes::new()` in Alloy encodes the same as empty calldata bytes in Solidity ABI encoding.

4. **Compare authorize vs release calldata**: authorize() works. Both take PaymentInfo as first param. Diff the ABI encoding of PaymentInfo between our authorize call (which works) and our release call (which fails). If they differ, that's the bug.

5. **Use Ali's SDK to call release**: Clone `x402r-sdk`, use `npx tsx` to call release with the same PaymentInfo via viem. If it works, diff the calldata against what our Rust code generates.

## Facilitator wallet

```
EVM Mainnet: 0x103040545AC5031A11E8C03dd11324C7333a13C7
Private key: AWS Secrets Manager `facilitator-evm-mainnet-private-key` (JSON: {"private_key": "...", "address": "..."})
```

## Current code (operator.rs execute_release)

```rust
let call = OperatorContract::releaseCall {
    paymentInfo: payment_info_abi,
    amount,
    data: alloy::primitives::Bytes::new(),
};
send_operator_tx(provider, target, &call).await
```

For CREATE3 networks (SKALE). Legacy networks use manual encoding with old selector `0xecf39b0a`.
