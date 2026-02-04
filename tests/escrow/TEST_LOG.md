# Escrow Scheme Integration Test Log

**Date**: 2026-02-02
**Network**: Base Mainnet (eip155:8453)
**Facilitator**: https://facilitator.ultravioletadao.xyz
**Status**: SUCCESS - Escrow scheme working on-chain!

---

## Summary

Testing the x402r Escrow Scheme integration. We discovered and solved THREE critical issues:

1. **SDK Nonce Bug**: The SDK's `computeEscrowNonce()` is WRONG - missing PAYMENT_INFO_TYPEHASH
2. **Architecture**: Must call through PaymentOperator (not directly to AuthCaptureEscrow)
3. **feeReceiver**: Must be set to PaymentOperator address (not feeRecipient)

**WORKING TX**: https://basescan.org/tx/0x5d831454e5577129d9f6b58f614bd9b1b0fe8b77cffca18d3ef5f7a7fc110678

---

## Deployed PaymentOperator (Base Mainnet)

```
Address: 0xa06958D93135BEd7e43893897C0d9fA931EF051C
TX: https://basescan.org/tx/0x65a022e67576682f94dad9d9ec82d8c58cccc16fd22c405b8545a7247c5efa60
Config:
  - feeRecipient: 0xD3868E1eD738CED6945A574a7c769433BeD5d474
  - feeCalculator: ZERO_ADDRESS (no custom fees)
  - All conditions/recorders: ZERO_ADDRESS (permissionless)
```

---

## Contract Addresses (Base Mainnet)

| Contract | Address |
|----------|---------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` |
| TokenStore (escrow) | `0x29BfE2143379Ca2E93721E42901610297f0AB463` |
| PaymentOperatorFactory | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` |
| **Our PaymentOperator** | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` |
| ProtocolFeeConfig | `0x230fd3A171750FA45db2976121376b7F47Cba308` |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |

---

## Test Environment

### Test Wallet
- Address: `0xD3868E1eD738CED6945A574a7c769433BeD5d474`
- ETH Balance: 0.009 ETH
- USDC Balance: 5.59 USDC
- Key stored in: AWS Secrets Manager `lighthouse-buyer-tester`

---

## CRITICAL FINDINGS

### Finding 1: SDK Nonce Computation is WRONG

**VERIFIED**: The x402r SDK computes nonces differently than the on-chain contract!

#### Contract's `getHash()` (AuthCaptureEscrow.sol:421-424):
```solidity
function getHash(PaymentInfo calldata paymentInfo) public view returns (bytes32) {
    bytes32 paymentInfoHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfo));
    return keccak256(abi.encode(block.chainid, address(this), paymentInfoHash));
}
```

#### SDK's nonce computation (nonce.ts):
```typescript
const encoded = encodeAbiParameters([chainId, escrow, paymentInfo], [...]);
return keccak256(encoded);  // WRONG - missing TYPEHASH!
```

**Contract uses TWO-STEP HASH:**
1. `keccak256(abi.encode(TYPEHASH, paymentInfo))`
2. `keccak256(abi.encode(chainId, escrow, paymentInfoHash))`

**SDK does single hash (WRONG):**
- `keccak256(abi.encode(chainId, escrow, paymentInfo))`

### Finding 2: PAYMENT_INFO_TYPEHASH

```
0xae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591

= keccak256("PaymentInfo(address operator,address payer,address receiver,address token,uint120 maxAmount,uint48 preApprovalExpiry,uint48 authorizationExpiry,uint48 refundExpiry,uint16 minFeeBps,uint16 maxFeeBps,address feeReceiver,uint256 salt)")
```

### Finding 3: Must Use PaymentOperator

`AuthCaptureEscrow.authorize()` has `onlySender(paymentInfo.operator)` modifier.
Direct calls fail because `msg.sender != paymentInfo.operator`.

**Solution**: Deploy a PaymentOperator and route calls through it:
- `paymentInfo.operator` = PaymentOperator address
- Anyone can call `PaymentOperator.authorize()` (permissionless)
- PaymentOperator calls AuthCaptureEscrow with correct `msg.sender`

### Finding 4: feeReceiver Must Be PaymentOperator Address

`PaymentInfo.feeReceiver` must be set to the PaymentOperator address itself:

```
Testing feeReceiver = payer: InvalidFeeReceiver
Testing feeReceiver = ZERO_ADDRESS: InvalidFeeReceiver
Testing feeReceiver = PaymentOperator: SUCCESS!
```

---

## Working Configuration

### PaymentInfo
```json
{
  "operator": "0xa06958D93135BEd7e43893897C0d9fA931EF051C",
  "payer": "<payer_address>",
  "receiver": "<receiver_address>",
  "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  "maxAmount": 10000,
  "preApprovalExpiry": "<unix_timestamp + 3600>",
  "authorizationExpiry": 281474976710655,
  "refundExpiry": 281474976710655,
  "minFeeBps": 0,
  "maxFeeBps": 100,
  "feeReceiver": "0xa06958D93135BEd7e43893897C0d9fA931EF051C",
  "salt": "<random_32_bytes>"
}
```

### ERC-3009 ReceiveWithAuthorization
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
    "from": "<payer>",
    "to": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
    "value": "<maxAmount>",
    "validAfter": 0,
    "validBefore": "<preApprovalExpiry>",
    "nonce": "<computed_nonce>"
  }
}
```

---

## Correct Nonce Computation (Python)

```python
PAYMENT_INFO_TYPEHASH = bytes.fromhex("ae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591")
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

def compute_nonce(chain_id, escrow_address, payment_info):
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

## Architecture Flow

```
Client (signs ERC-3009 ReceiveWithAuthorization)
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
    v (calls USDC.receiveWithAuthorization)
USDC Contract
    |
    v (transfers from payer to TokenCollector)
TokenCollector
    |
    v (transfers to escrow)
TokenStore (holds escrowed funds)
```

---

## Test Results

### Successful Escrow Authorize TX
**TX**: https://basescan.org/tx/0x5d831454e5577129d9f6b58f614bd9b1b0fe8b77cffca18d3ef5f7a7fc110678

```
Status: SUCCESS
Gas used: 203,406
Block: 41646333
Events:
  1. PaymentOperator.Authorized
  2. AuthCaptureEscrow.Authorized
  3. USDC.AuthorizationUsed
  4. USDC.Transfer (payer -> TokenCollector)
  5. USDC.Transfer (TokenCollector -> TokenStore)
Result: 0.01 USDC moved to escrow
```

### Verification
```
Payer USDC balance: 5.59 USDC (was 5.6)
TokenStore balance: 0.01 USDC
```

---

## Test Scripts

| Script | Purpose | Status |
|--------|---------|--------|
| `test_direct_authorize.py` | Direct on-chain escrow test | WORKING |
| `deploy_operator.py` | Deploy PaymentOperator via factory | WORKING |
| `verify_onchain_hash.py` | Compare SDK vs on-chain nonce | WORKING |
| `test_escrow_with_correct_nonce.py` | Test via facilitator | WORKING |

---

## Facilitator Integration Status

### Completed (2026-02-02)

1. [x] Add escrow scheme handler to `/settle` endpoint (`src/handlers.rs`)
2. [x] Add PaymentOperator.authorize() call support (`src/payment_operator/operator.rs`)
3. [x] Configure PaymentOperator address per network (`src/payment_operator/addresses.rs`)
4. [x] Add escrow scheme to `/supported` endpoint (`src/facilitator_local.rs`)
5. [x] Add `Scheme::Escrow` enum variant (`src/types.rs`)
6. [x] Add `EscrowSupportedInfo` type for /supported response (`src/types.rs`)
7. [x] Add `ENABLE_PAYMENT_OPERATOR=true` to Terraform (`terraform/environments/production/main.tf`)
8. [x] Update `.env.example` with correct PaymentOperator info
9. [x] **Build and deploy** v1.24.0-escrow to production
10. [x] **Run integration test** via facilitator - SUCCESS!

### Successful Facilitator Test TX

**TX**: https://basescan.org/tx/0x4fd9be51aac43c7492062cc247a32304e3ddb68cbbcf9a2e8e0faf95b7adbfef

```
Status: SUCCESS
Network: Base Mainnet (eip155:8453)
Facilitator: https://facilitator.ultravioletadao.xyz
Test Script: test_escrow_with_correct_nonce.py
```

### Future Work

1. [ ] Test charge/release/refund flows
2. [ ] Add Base Sepolia testnet support

### Critical Fix Applied

Fixed signature type in test script: Changed from `TransferWithAuthorization` to `ReceiveWithAuthorization`
because TokenCollector calls `USDC.receiveWithAuthorization()`, not `transferWithAuthorization()`.

---

## For Ali's Team

1. **SDK Bug Report**: The `computeEscrowNonce()` in `packages/evm/src/shared/nonce.ts` is missing PAYMENT_INFO_TYPEHASH
2. **Reported via chat on 2026-02-02**

---

## Repository Analysis

Analyzed these x402r repositories:
- https://github.com/BackTrackCo/x402r-sdk
- https://github.com/BackTrackCo/x402r-scheme
- https://github.com/BackTrackCo/x402r-contracts

### Key Files
| File | Purpose |
|------|---------|
| `x402r-contracts/lib/commerce-payments/src/AuthCaptureEscrow.sol` | On-chain getHash() (source of truth) |
| `x402r-scheme/packages/evm/src/shared/nonce.ts` | SDK nonce computation (BUGGY) |
| `x402r-contracts/lib/commerce-payments/src/collectors/TokenCollector.sol` | _getHashPayerAgnostic() |
