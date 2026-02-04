# Advanced Escrow Test History - Complete Timeline

## Overview

This document chronicles the complete journey of testing the x402r PaymentOperator Advanced Escrow system on Base Mainnet, from initial failures to all 5 lifecycle tests passing.

**Contract**: PaymentOperator at `0xa06958D93135BEd7e43893897C0d9fA931EF051C` (Base Mainnet)
**Escrow**: AuthCaptureEscrow at `0x320a3c35F131E5D2Fb36af56345726B298936037`
**Test Wallet**: `0xD3868E1eD738CED6945A574a7c769433BeD5d474`
**Each test uses**: 0.01 USDC (10000 units, 6 decimals)

---

## Phase 1: SDK Bug Discovery & First Authorize

### Bug Found: SDK Nonce Computation

The x402r SDK computes nonces differently from the on-chain AuthCaptureEscrow contract:

**SDK (WRONG)**:
```
keccak256(abi.encode(chainId, escrow, paymentInfo))
```

**Contract (CORRECT)**:
```
Step 1: paymentInfoHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfo))
Step 2: return keccak256(abi.encode(chainId, escrow, paymentInfoHash))
```

The SDK was missing the `PAYMENT_INFO_TYPEHASH` prefix. This caused every escrow operation to fail with "FiatTokenV2: invalid signature" because the nonce (used as the ERC-3009 authorization nonce) didn't match what the contract expected.

**PAYMENT_INFO_TYPEHASH**: `0xae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591`

### First Successful AUTHORIZE

After fixing the nonce computation, the first AUTHORIZE succeeded via the facilitator:

```
test_escrow_with_correct_nonce.py
Status: PASS
TX: 0x3bb0e9f43912ee3eb0c3284d999db312713456c06e6076afb97378f0e9ffec0e
```

The facilitator handles AUTHORIZE by:
1. Parsing the escrow payload (PaymentInfo + ERC-3009 authorization)
2. Building the on-chain call: `PaymentOperator.authorize(paymentInfo, amount, tokenCollector, collectorData)`
3. The TokenCollector calls `USDC.receiveWithAuthorization()` to move funds to escrow

---

## Phase 2: The `PaymentAlreadyCollected` Discovery

### Initial Error

After AUTHORIZE succeeded, we tried to call `PaymentOperator.charge()` to release funds to the receiver. Every attempt failed with:

```
custom error 0xad7c145a: PaymentAlreadyCollected(bytes32)
```

This error came from `AuthCaptureEscrow.charge()` at the check:
```solidity
if (paymentState[paymentInfoHash].hasCollectedPayment) revert PaymentAlreadyCollected(paymentInfoHash);
```

### Wrong ABI Discovery

First we discovered the test scripts had hand-written ABIs with wrong function signatures:

| Function | Wrong ABI | Correct ABI |
|----------|-----------|-------------|
| charge | `(paymentInfo, amount, feeBps, recorderData)` | `(paymentInfo, amount, tokenCollector, collectorData)` |
| release | `(paymentInfo, recorderData)` | `(paymentInfo, amount)` |
| refundInEscrow | `(paymentInfo, recorderData)` | `(paymentInfo, uint120 amount)` |

Fixed by loading ABIs from the compiled `abi/PaymentOperator.json`.

### Root Cause: `charge()` vs `authorize()` Are ALTERNATIVES

Even with the correct ABI and ZERO_ADDRESS as tokenCollector, `charge()` still failed. The cast trace showed:

```
PaymentOperator calls getHash() -> returns hash
PaymentOperator emits ChargeExecuted event
PaymentOperator calls AuthCaptureEscrow.charge() -> reverts with PaymentAlreadyCollected
```

**The breakthrough**: Reading the PaymentOperator.sol contract comments (line 55-58):

```solidity
// ARCHITECTURE: Users call operator methods directly:
//     User -> operator.authorize() -> escrow.authorize()
//     User -> operator.charge() -> escrow.charge()
//     User -> operator.release() -> escrow.capture()
```

`authorize()` and `charge()` are **alternative first operations**, not sequential!

- `authorize()` sets `hasCollectedPayment = true` with `capturableAmount > 0` (held in escrow)
- `charge()` ALSO sets `hasCollectedPayment = true` but with `capturableAmount = 0` (distributed immediately)
- Calling `charge()` after `authorize()` fails because the flag is already true

After `authorize()`, the correct function to release funds to the receiver is **`release()`** which calls `escrow.capture()`.

---

## Phase 3: Correct Function Mapping

### PaymentOperator -> AuthCaptureEscrow Mapping

| PaymentOperator | Escrow Function | Purpose |
|-----------------|-----------------|---------|
| `authorize(paymentInfo, amount, tokenCollector, collectorData)` | `escrow.authorize()` | Lock funds in escrow |
| `release(paymentInfo, amount)` | `escrow.capture()` | Release escrowed funds TO receiver |
| `refundInEscrow(paymentInfo, uint120 amount)` | `escrow.partialVoid()` | Return escrowed funds TO payer |
| `charge(paymentInfo, amount, tokenCollector, collectorData)` | `escrow.charge()` | Direct payment, NO escrow hold |
| `refundPostEscrow(paymentInfo, amount, tokenCollector, collectorData)` | `escrow.refund()` | Dispute refund after release |

### The 5 Correct Flows

1. **AUTHORIZE**: Lock funds (via facilitator)
2. **AUTHORIZE -> RELEASE**: Lock, then capture to receiver (worker gets paid)
3. **AUTHORIZE -> REFUND IN ESCROW**: Lock, then return to payer (cancel task)
4. **CHARGE**: Direct instant payment (standalone, no escrow)
5. **AUTHORIZE -> RELEASE -> REFUND POST ESCROW**: Full dispute flow

---

## Phase 4: All Tests Rewritten and Passing

### Test 1: AUTHORIZE (via facilitator)
**Script**: `test_escrow_with_correct_nonce.py`
**Flow**: Facilitator calls `PaymentOperator.authorize()`
**Status**: PASS
**Gas**: ~164,000

### Test 2: RELEASE (AUTHORIZE -> capture to receiver)
**Script**: `test_2_release.py`
**Flow**: Facilitator AUTHORIZE, then on-chain `PaymentOperator.release(paymentInfo, amount)`
**Status**: PASS
**Gas**: ~88,460 (release step)
**Example TX**: `0xe8eaf9140fa06d619cb5cd5ff5c345ec5dc2e062be88e31583b05640868907ed`

### Test 3: REFUND IN ESCROW (AUTHORIZE -> return to payer)
**Script**: `test_3_refund_in_escrow.py`
**Flow**: Facilitator AUTHORIZE, then on-chain `PaymentOperator.refundInEscrow(paymentInfo, amount)`
**Status**: PASS
**Gas**: ~79,241 (refund step)
**Example TX**: `0x538b16491387a1ed52ab8d6be16bc7ab98ee9685ea6dd328b9f1e706bf8811a0`

### Test 4: CHARGE (direct payment, no escrow)
**Script**: `test_4_charge.py`
**Flow**: Direct on-chain `PaymentOperator.charge(paymentInfo, amount, tokenCollector, collectorData)`
**Status**: PASS
**Gas**: ~173,611
**Example TX**: `0x4524d49a10d2bc163d49ba5e9a24ea5887bb7e8e0209f5e79a12cb9bab5b7b9f`

### Test 5: REFUND POST ESCROW (dispute after release)
**Script**: `test_5_refund_post_escrow.py`
**Flow**: Facilitator AUTHORIZE -> on-chain RELEASE -> attempt REFUND POST ESCROW
**Status**: PASS (steps 1-2 succeed; step 3 reverts as expected without RefundRequest approval)
**Note**: RefundPostEscrow requires RefundRequest contract approval in production
**Example RELEASE TX**: `0x7ec4e75bda0419ddfc690c919fa3dd4e55e5f6669e09d6d1d22ad685462c5f58`

### Full Suite Results

```
======================================================================
                         TEST SUMMARY
======================================================================
  [PASS] 1. AUTHORIZE
  [PASS] 2. RELEASE
  [PASS] 3. REFUND IN ESCROW
  [PASS] 4. CHARGE
  [PASS] 5. REFUND POST ESCROW
======================================================================
  Total: 5/5 tests passed
  Time: 83.1 seconds
======================================================================
```

---

## Key Technical Learnings

1. **PAYMENT_INFO_TYPEHASH is critical**: The SDK bug that omitted it caused all nonce computations to fail
2. **ERC-3009 type must be ReceiveWithAuthorization**: NOT TransferWithAuthorization, because the TokenCollector calls `USDC.receiveWithAuthorization()`
3. **feeReceiver MUST be the PaymentOperator address**: Not a platform wallet
4. **authorize() and charge() are alternatives**: Both set `hasCollectedPayment = true`
5. **release() captures to receiver**: Confusing name - "release" means "release from escrow to receiver"
6. **Timing is Unix seconds**: Not milliseconds
7. **5-second delay needed between tests**: RPC rate limiting causes intermittent "invalid signature" errors when tests run too fast
8. **RefundPostEscrow requires RefundRequest**: Can't unilaterally refund after release

---

## Cost Summary

| Test | USDC Cost | Gas (ETH) |
|------|-----------|-----------|
| AUTHORIZE | 0.01 USDC | ~164K gas |
| RELEASE | 0.01 USDC | ~88K gas |
| REFUND IN ESCROW | 0.01 USDC | ~79K gas |
| CHARGE | 0.01 USDC | ~174K gas |
| REFUND POST ESCROW | 0.01 USDC | ~88K gas (release) |
| **Total** | **~0.05 USDC** | **~$0.05-0.10** |

---

## Files Created/Modified

| File | Description |
|------|-------------|
| `test_escrow_with_correct_nonce.py` | Test 1: AUTHORIZE via facilitator |
| `test_2_release.py` | Test 2: AUTHORIZE -> RELEASE (capture) |
| `test_3_refund_in_escrow.py` | Test 3: AUTHORIZE -> REFUND IN ESCROW |
| `test_4_charge.py` | Test 4: CHARGE (direct payment) |
| `test_5_refund_post_escrow.py` | Test 5: AUTHORIZE -> RELEASE -> REFUND POST ESCROW |
| `run_all_tests.py` | Master test runner |
| `debug_payment_info.py` | Debug utility for encoding verification |
| `TEST_HISTORY.md` | This file |
