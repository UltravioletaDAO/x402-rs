Hey Ali,

Detailed debug report on the SKALE operator issue. We need your help figuring out why `release()` reverts.

## The Problem

PaymentOperators deployed on SKALE Base via your Shanghai factory (`0x3Cd5c76F...`) can `authorize()` but cannot `release()`. Two separate operators were deployed independently and both exhibit the same behavior.

## Operators Compared

| Property | Base 0x271f (working) | SKALE 0x942c (ours) | SKALE 0x28c2 (EM) |
|----------|----------------------|--------------------|--------------------|
| Factory | Old Cancun factory | `0x3Cd5c76F...` (Shanghai) | `0x3Cd5c76F...` (Shanghai) |
| Bytecode size | 15,303 chars | 15,523 chars | 15,523 chars |
| authorize() | Works | Works | Works |
| release() | Works (reverts with `InvalidSender` when called incorrectly) | Reverts with no error message | Reverts with no error message |
| charge() | Works | Reverts with no error message | Reverts with no error message |

Key observation: **Base reverts with a decodable custom error** (`InvalidSender(address,uint256)`) while **SKALE reverts with no message at all**. This could mean:
1. The function exists but hits an opcode SKALE doesn't support during execution
2. The function was compiled differently and doesn't match the expected selector
3. SKALE's error reporting is different

## On-Chain Configs

**Operator 0x942c (our marketplace test):**
```
feeRecipient:           0x103040545AC5031A11E8C03dd11324C7333a13C7 (facilitator)
feeCalculator:          0x0000000000000000000000000000000000000000 (none)
authorizeCondition:     0x96a585F0e23eE9FD8722C7a61d3b8B3FAd2419df (usdcTvlLimit)
releaseCondition:       0x6f49092b9cC961587515B1C280a924277e830B9D (EscrowPeriod, 7 days)
refundInEscrowCondition: 0x43866c57577D76C590aA3ca3370ab6d1C611bd3A (OrCondition)
```

**Operator 0x28c2 (EM production):**
```
feeRecipient:           0xaE07cEB6b395BC685a776a0b4c489E8d9cE9A6ad (EM wallet)
feeCalculator:          0xC5eE05f8CCaf1fC535624F55F03AF41532A8E4da (1300bps)
releaseCondition:       0xAEC7955819Fa97c28C936A5A4b4E301385DA97B0 (OR payer|facilitator)
refundInEscrowCondition: 0xBd6B8C5a182e8C3B6C8624453F84ad0C899D30eF (facilitator-only)
```

## Escrow State

```
AuthCaptureEscrow:       0xBC151792f80C0EB1973d56b0235e6bee2A60e245
TokenStore(USDC.e):      0x89A30d6CaAa9DA1fdd1948b986255Cf96065A9Ce
TokenStoreImplementation: 0x16a485fd6F7BDC0258021e1Be23E7C033FD9d4d9
```

EM successfully locked 0.10 USDC across 2 lockboxes via `authorize()`. The USDC is in the TokenStore.

## What We Need From You

1. **Debug question**: Does the Shanghai-compiled PaymentOperator have `release()` in its ABI? Or did the Shanghai downgrade strip it? Can you compare the Solidity compiler output for Cancun vs Shanghai to see which functions got removed?

2. **Recovery**: Is there a way to release/void the locked funds without going through the operator's `release()`? For example, does the AuthCaptureEscrow contract have a `reclaim()`, `void()`, or timeout mechanism that works independently?

3. **Fix**: Can you compile a PaymentOperator that works on Shanghai EVM with ALL functions intact? The issue might be that `ReentrancyGuardTransient` (TSTORE/TLOAD) is used in `release()` but not in `authorize()`, which is why authorize works but release doesn't.

4. **Alternative**: If full Shanghai support isn't possible, what's the minimum we need from SKALE's EVM upgrade? Just EIP-1153 (TSTORE/TLOAD)?

## Stuck Funds

- 0.05 USDC in lockbox `0x1e8d4531174f47093e962d72700c0983f48ebab6`
- 0.05 USDC in another lockbox
- Both via operator 0x28c2 on SKALE Base
- EM needs these released to complete the Execution Market test

## Files for Reference

- Your SDK branch: `A1igator/sync-abis-data-param`
- Your config: `packages/core/src/config/index.ts`
- Our facilitator: v1.40.4 with SKALE escrow support
- SKALE chain ID: 1187947933

## UPDATE: Possible ABI Mismatch (not missing function)

We discovered that the release function signature CHANGED between your old and new deployments:

- **Old ABI** (Base operator 0x271f): `release(PaymentInfo, uint256)` = selector `0xecf39b0a`
- **New ABI** (SKALE operator 0x28c2): `release(PaymentInfo, uint256, bytes data)` = selector `0xc602dd4a`

Your new SDK ABI (`packages/core/src/abis/generated.ts`) shows release() takes 3 params including `bytes data`. The old contracts on Base only take 2 params.

**The Execution Market is calling the old selector `0xecf39b0a`** on the new SKALE operator. The function doesn't match because the signature changed. This might not be a Shanghai/Cancun issue at all -- it might just be an ABI version mismatch.

**Can you confirm**: Does the SKALE PaymentOperator have `release(PaymentInfo, uint256, bytes)` at selector `0xc602dd4a`? If yes, EM just needs to update to the new ABI and use the 3-param version.

Thanks Ali -- this is the last piece to get the full Execution Market running on SKALE.
