# x402r Escrow Scheme Integration - Critical Findings Summary

**Date**: 2026-02-02
**Author**: Claude Code analysis
**Status**: SUCCESS - Escrow scheme working on Base Mainnet!

---

## Executive Summary

After deep analysis of the x402r SDK, contracts, and our facilitator integration, we discovered THREE critical issues and successfully tested the escrow scheme:

1. **SDK Bug**: The `computeEscrowNonce()` function in the SDK is WRONG (reported to Ali)
2. **Architecture**: Must call through a deployed PaymentOperator (not directly to AuthCaptureEscrow)
3. **feeReceiver**: Must be set to the PaymentOperator address (not feeRecipient)

**WORKING TEST TX**: https://basescan.org/tx/0x5d831454e5577129d9f6b58f614bd9b1b0fe8b77cffca18d3ef5f7a7fc110678

---

## Deployed PaymentOperator (Base Mainnet)

```
Address: 0xa06958D93135BEd7e43893897C0d9fA931EF051C
Config:
  - feeRecipient: 0xD3868E1eD738CED6945A574a7c769433BeD5d474
  - feeCalculator: ZERO_ADDRESS (no custom fees)
  - All conditions/recorders: ZERO_ADDRESS (permissionless)
```

---

## Issue 1: SDK Nonce Computation Bug

### Problem
The x402r SDK's nonce computation doesn't match the on-chain contract.

### Evidence
```
On-chain getHash():     0x8278d8424034803841e39468fa9458fc21006bd8c90078d9c023fd2905347a9e
SDK computeEscrowNonce: 0xc8f0cfb3669d0f6d9c7228637109267645a55a9de4ba6d401b2d474748a6872e
```

### Root Cause
The contract (AuthCaptureEscrow.sol:421-424) computes:
```solidity
function getHash(PaymentInfo calldata paymentInfo) public view returns (bytes32) {
    bytes32 paymentInfoHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfo));
    return keccak256(abi.encode(block.chainid, address(this), paymentInfoHash));
}
```

The SDK (nonce.ts) computes:
```typescript
const encoded = encodeAbiParameters([chainId, escrow, paymentInfo], [...]);
return keccak256(encoded);
```

**The SDK is missing the PAYMENT_INFO_TYPEHASH in the first hash step!**

### PAYMENT_INFO_TYPEHASH
```
0xae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591

= keccak256("PaymentInfo(address operator,address payer,address receiver,address token,uint120 maxAmount,uint48 preApprovalExpiry,uint48 authorizationExpiry,uint48 refundExpiry,uint16 minFeeBps,uint16 maxFeeBps,address feeReceiver,uint256 salt)")
```

### Working Python Implementation
```python
def compute_correct_nonce(chain_id, escrow_address, payment_info):
    # Create tuple with payer = 0 (payer-agnostic)
    payment_info_tuple = (
        payment_info['operator'],
        ZERO_ADDRESS,  # payer = 0
        payment_info['receiver'],
        payment_info['token'],
        payment_info['maxAmount'],
        payment_info['preApprovalExpiry'],
        payment_info['authorizationExpiry'],
        payment_info['refundExpiry'],
        payment_info['minFeeBps'],
        payment_info['maxFeeBps'],
        payment_info['feeReceiver'],
        payment_info['salt'],
    )

    # Step 1: keccak256(abi.encode(TYPEHASH, paymentInfo))
    encoded = encode(
        ['bytes32', '(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)'],
        [PAYMENT_INFO_TYPEHASH, payment_info_tuple],
    )
    payment_info_hash = keccak256(encoded)

    # Step 2: keccak256(abi.encode(chainId, escrow, paymentInfoHash))
    final = encode(['uint256', 'address', 'bytes32'], [chain_id, escrow_address, payment_info_hash])
    return keccak256(final)
```

---

## Issue 2: Must Use PaymentOperator

### Problem
The `AuthCaptureEscrow.authorize()` function has an `onlySender(paymentInfo.operator)` modifier.
Direct calls from facilitator wallet fail because `msg.sender != paymentInfo.operator`.

### Solution
Deploy a PaymentOperator and route calls through it:
- `paymentInfo.operator` = PaymentOperator address
- Facilitator calls `PaymentOperator.authorize()`
- PaymentOperator calls `AuthCaptureEscrow.authorize()` with correct msg.sender

### Architecture
```
Client (signs ERC-3009)
    |
    v
Facilitator (calls PaymentOperator.authorize)
    |
    v (permissionless, anyone can call)
PaymentOperator.authorize()
    |
    v (msg.sender = PaymentOperator = paymentInfo.operator)
AuthCaptureEscrow.authorize()
    |
    v
TokenCollector.collectTokens()
    |
    v (calls USDC.receiveWithAuthorization with computed nonce)
USDC Contract
    |
    v (transfers from payer to TokenCollector)
TokenCollector
    |
    v (transfers to escrow)
TokenStore (holds escrowed funds)
```

---

## Issue 3: feeReceiver Must Be PaymentOperator Address

### Problem
`PaymentInfo.feeReceiver` must be set to the PaymentOperator address itself, NOT the feeRecipient configured in the operator.

### Discovery
```
Testing feeReceiver = payer (same as feeRecipient): InvalidFeeReceiver
Testing feeReceiver = ZERO_ADDRESS: InvalidFeeReceiver
Testing feeReceiver = PaymentOperator: SUCCESS!
Testing feeReceiver = AuthCaptureEscrow: InvalidFeeReceiver
```

### Solution
Always set `paymentInfo.feeReceiver = paymentInfo.operator` (the PaymentOperator address)

---

## Working PaymentInfo Configuration

```json
{
  "operator": "0xa06958D93135BEd7e43893897C0d9fA931EF051C",
  "payer": "<payer_address>",
  "receiver": "<receiver_address>",
  "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  "maxAmount": 10000,
  "preApprovalExpiry": <unix_timestamp + 3600>,
  "authorizationExpiry": 281474976710655,
  "refundExpiry": 281474976710655,
  "minFeeBps": 0,
  "maxFeeBps": 100,
  "feeReceiver": "0xa06958D93135BEd7e43893897C0d9fA931EF051C",
  "salt": "<random_32_bytes>"
}
```

**Key points:**
- `operator` = PaymentOperator address (NOT factory, NOT facilitator wallet)
- `feeReceiver` = PaymentOperator address (MUST match operator)
- `preApprovalExpiry` = Unix timestamp in seconds (NOT milliseconds, NOT MAX_UINT48 for short tests)

---

## ERC-3009 Signature Parameters

Sign `ReceiveWithAuthorization` (not `TransferWithAuthorization`):

```json
{
  "domain": {
    "name": "USD Coin",
    "version": "2",
    "chainId": 8453,
    "verifyingContract": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  },
  "types": {
    "ReceiveWithAuthorization": [
      {"name": "from", "type": "address"},
      {"name": "to", "type": "address"},
      {"name": "value", "type": "uint256"},
      {"name": "validAfter", "type": "uint256"},
      {"name": "validBefore", "type": "uint256"},
      {"name": "nonce", "type": "bytes32"}
    ]
  },
  "message": {
    "from": "<payer_address>",
    "to": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
    "value": 10000,
    "validAfter": 0,
    "validBefore": <preApprovalExpiry>,
    "nonce": "<computed_nonce>"
  }
}
```

**Key points:**
- `to` = TokenCollector address (NOT escrow, NOT operator, NOT receiver)
- `value` = maxAmount from PaymentInfo
- `validBefore` = preApprovalExpiry from PaymentInfo
- `nonce` = computed using PAYMENT_INFO_TYPEHASH (see above)

---

## Test Scripts

| Script | Purpose |
|--------|---------|
| `test_direct_authorize.py` | Direct on-chain escrow test (WORKING!) |
| `deploy_operator.py` | Deploy PaymentOperator via factory |
| `verify_onchain_hash.py` | Compare SDK vs on-chain nonce |
| `test_escrow_with_correct_nonce.py` | Test via facilitator (needs integration) |

---

## Contract Addresses (Base Mainnet)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenStore | `0x29BfE2143379Ca2E93721E42901610297f0AB463` |
| PaymentOperatorFactory | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| **Our PaymentOperator** | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` |
| ProtocolFeeConfig | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |

---

## Next Steps

### For Ali's Team
1. **Fix SDK nonce computation** - Add PAYMENT_INFO_TYPEHASH to hash (reported)

### For Our Facilitator
1. Add escrow scheme handler to facilitator
2. Implement correct nonce computation in Rust
3. Use our deployed PaymentOperator (0xa06958D93135BEd7e43893897C0d9fA931EF051C)
4. Test charge/release/refund flows

---

## Success Evidence

**TX**: https://basescan.org/tx/0x5d831454e5577129d9f6b58f614bd9b1b0fe8b77cffca18d3ef5f7a7fc110678

```
Status: SUCCESS
Gas used: 203,406
Block: 41646333
Events: 5 (PaymentOperator.Authorized, AuthCaptureEscrow.Authorized, USDC.AuthorizationUsed, USDC.Transfer x2)
Result: 0.01 USDC moved from payer to TokenStore (escrow)
```
