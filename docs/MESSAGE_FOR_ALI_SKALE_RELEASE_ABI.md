Hey Ali,

We deployed operators on SKALE Base using your Shanghai factory and hit an issue with `release()`. After debugging, we think we found the root cause -- an ABI change between your old and new contract versions.

## What Works

- `authorize()` on SKALE operators -- works perfectly, funds go into escrow
- All factory deploy() calls work
- Escrow contract view functions work

## What Doesn't Work

- `release()` reverts on SKALE operators
- We have 0.10 USDC locked in escrow from Execution Market tests

## Root Cause: release() Signature Changed

Your new SDK ABI (`packages/core/src/abis/generated.ts` on `A1igator/sync-abis-data-param`) shows:

```
release(PaymentInfo, uint256 amount, bytes data)  -->  selector 0xc602dd4a
```

But the old operators on Base (0x271f) have:

```
release(PaymentInfo, uint256 amount)  -->  selector 0xecf39b0a
```

We verified on-chain:

| Selector | Base 0x271f | SKALE 0x28c2 |
|----------|-------------|--------------|
| `0xecf39b0a` (old, 2 params) | EXISTS (returns `InvalidSender` error) | No error data (not recognized) |
| `0xc602dd4a` (new, 3 params) | Not recognized | Unclear (SKALE doesn't return error data) |

The Execution Market is calling `0xecf39b0a` (old 2-param release) on the SKALE operator which was deployed with the new factory that uses the 3-param version.

## What We Need

Confirm: Does the SKALE PaymentOperator have `release(PaymentInfo, uint256, bytes)` at `0xc602dd4a`? If yes, EM just needs to update their ABI to the new 3-param signature.

## Operators on SKALE

```
0x942c (ours):
  feeRecipient: 0x103040545AC5031A11E8C03dd11324C7333a13C7
  feeCalculator: 0x0 (none)
  authorizeCondition: 0x96a585F0... (usdcTvlLimit)
  releaseCondition: 0x6f49092b... (EscrowPeriod 7d)
  refundInEscrowCondition: 0x43866c57... (OrCondition)

0x28c2 (EM):
  feeRecipient: 0xaE07cEB6b395BC685a776a0b4c489E8d9cE9A6ad
  feeCalculator: 0xC5eE05f8... (1300bps)
  releaseCondition: 0xAEC79558... (OR payer|facilitator)
  refundInEscrowCondition: 0xBd6B8C5a... (facilitator-only)
```

## Escrow State

```
AuthCaptureEscrow: 0xBC151792f80C0EB1973d56b0235e6bee2A60e245
TokenStore(USDC.e): 0x89A30d6CaAa9DA1fdd1948b986255Cf96065A9Ce
```

Thanks!
