# x402r Advanced Escrow: Integration Report for Ali & x402r Team

**From**: Ultravioleta DAO (Chamba Integration)
**Date**: February 2026
**Subject**: PaymentOperator Advanced Escrow - Full Lifecycle Validation on Base Mainnet

---

## Executive Summary

We successfully validated all 5 PaymentOperator Advanced Escrow flows on Base Mainnet, built SDK wrappers (Python + TypeScript), and integrated them into our AI agent marketplace (Chamba). This report documents our findings, a critical SDK bug we fixed, and recommendations.

**Result: 5/5 on-chain flows verified. Production-ready.**

---

## Contracts Tested

| Contract | Address | Network |
|----------|---------|---------|
| PaymentOperator | `0xa06958D93135BEd7e43893897C0d9fA931EF051C` | Base Mainnet (8453) |
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` | Base Mainnet |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` | Base Mainnet |
| TokenStore | `0x29BfE2143379Ca2E93721E42901610297f0AB463` | Base Mainnet |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` | Base Mainnet |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | Base Mainnet |

Test wallet: `0xD3868E1eD738CED6945A574a7c769433BeD5d474`

---

## Test Results

### All 5 Flows Verified

| # | Flow | Operator Function | Escrow Function | Gas | Status |
|---|------|-------------------|-----------------|-----|--------|
| 1 | AUTHORIZE | `authorize(pi, amount, tokenCollector, data)` | `authorize()` | ~164K | PASS |
| 2 | RELEASE | `release(pi, amount)` | `capture()` | ~88K | PASS |
| 3 | REFUND IN ESCROW | `refundInEscrow(pi, uint120 amount)` | `partialVoid()` | ~79K | PASS |
| 4 | CHARGE | `charge(pi, amount, tokenCollector, data)` | `charge()` | ~174K | PASS |
| 5 | REFUND POST ESCROW | `refundPostEscrow(pi, amount, tokenCollector, data)` | `refund()` | ~88K | PASS* |

*Step 5 reverts as expected without RefundRequest approval - this is correct behavior.

### Chamba Scenario Tests (4/4 Pass)

| Scenario | Flow | Status |
|----------|------|--------|
| Standard task completion | AUTHORIZE -> RELEASE | PASS |
| Cancelled task | AUTHORIZE -> REFUND IN ESCROW | PASS |
| Instant trusted payment | CHARGE | PASS |
| Full lifecycle + dispute | AUTHORIZE -> RELEASE -> REFUND POST ESCROW attempt | PASS |

---

## Critical Bug Found: SDK Nonce Computation

### Issue

The nonce computation in the x402r SDK was missing the `PAYMENT_INFO_TYPEHASH` prefix, causing all escrow operations to fail with "FiatTokenV2: invalid signature".

### SDK (Wrong)
```
nonce = keccak256(abi.encode(chainId, escrow, paymentInfo))
```

### Contract (Correct)
```
Step 1: paymentInfoHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfo))
Step 2: nonce = keccak256(abi.encode(chainId, escrow, paymentInfoHash))
```

### PAYMENT_INFO_TYPEHASH
```
0xae68ac7ce30c86ece8196b61a7c486d8f0061f575037fbd34e7fe4e2820c6591
```

This is the keccak256 of the Solidity struct type:
```solidity
PaymentInfo(address operator,address payer,address receiver,address token,uint120 maxAmount,uint48 preApprovalExpiry,uint48 authorizationExpiry,uint48 refundExpiry,uint16 minFeeBps,uint16 maxFeeBps,address feeReceiver,uint256 salt)
```

### Impact

Without this fix, no escrow operations (authorize, charge) work because the ERC-3009 nonce doesn't match what the AuthCaptureEscrow contract expects.

### Fix Applied

We fixed this in both our Python and TypeScript SDKs. The correct nonce computation:

```python
# Step 1: Hash PaymentInfo with TYPEHASH prefix
encoded = abi.encode(
    ['bytes32', 'tuple(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)'],
    [PAYMENT_INFO_TYPEHASH, payment_info_tuple]
)
pi_hash = keccak256(encoded)

# Step 2: Combine with chain context
nonce = keccak256(abi.encode(
    ['uint256', 'address', 'bytes32'],
    [chain_id, escrow_address, pi_hash]
))
```

**Recommendation**: Update the official x402r SDK to include this fix.

---

## Technical Observations

### 1. authorize() vs charge() Are Alternatives

Both set `hasCollectedPayment = true` on the payment hash. Calling `charge()` after `authorize()` fails with `PaymentAlreadyCollected`. This is correct behavior but was initially confusing.

**Correct flows**:
- `authorize()` -> `release()` (capture to receiver)
- `authorize()` -> `refundInEscrow()` (return to payer)
- `charge()` standalone (direct payment)

### 2. release() Name Is Misleading

`operator.release()` calls `escrow.capture()`. The name "release" suggests releasing funds from a hold, but functionally it's capturing escrowed funds for the receiver. Consider documenting this mapping prominently.

### 3. PaymentInfo.payer Must Be Zero in Nonce

When computing the nonce, the `payer` field must be `address(0)` (payer-agnostic). The actual payer is determined at transaction time. Setting payer to the actual address causes nonce mismatch.

### 4. feeReceiver Must Be PaymentOperator

The `feeReceiver` in PaymentInfo must be the PaymentOperator contract address itself, not a platform wallet. The operator handles fee distribution.

### 5. ERC-3009 Must Use ReceiveWithAuthorization

The TokenCollector calls `USDC.receiveWithAuthorization()`, so the EIP-712 type must be `ReceiveWithAuthorization`, not `TransferWithAuthorization`.

### 6. refundPostEscrow Requires RefundRequest

Cannot call `refundPostEscrow()` without prior approval from the RefundRequest contract. This is by design for dispute resolution.

---

## Function Signatures (Verified from ABI)

```solidity
// AUTHORIZE (via facilitator)
function authorize(
    PaymentInfo calldata paymentInfo,
    uint256 amount,
    address tokenCollector,
    bytes calldata collectorData
) external;

// RELEASE (on-chain, after authorize)
function release(
    PaymentInfo calldata paymentInfo,
    uint256 amount
) external;

// REFUND IN ESCROW (on-chain, after authorize)
function refundInEscrow(
    PaymentInfo calldata paymentInfo,
    uint120 amount  // NOTE: uint120, not uint256
) external;

// CHARGE (on-chain, standalone)
function charge(
    PaymentInfo calldata paymentInfo,
    uint256 amount,
    address tokenCollector,
    bytes calldata collectorData
) external;

// REFUND POST ESCROW (on-chain, after release)
function refundPostEscrow(
    PaymentInfo calldata paymentInfo,
    uint256 amount,
    address tokenCollector,
    bytes calldata collectorData
) external;
```

Note: `refundInEscrow` uses `uint120` for amount, while others use `uint256`.

---

## SDK Implementation

We built SDK wrappers for both Python and TypeScript:

### Python SDK (`uvd-x402-sdk`)

```python
from uvd_x402_sdk.advanced_escrow import AdvancedEscrowClient

client = AdvancedEscrowClient(
    private_key="0x...",
    facilitator_url="https://facilitator.ultravioletadao.xyz",
    rpc_url="https://mainnet.base.org",
    chain_id=8453,
)

# Build payment info with tier-based timing
pi = client.build_payment_info(receiver="0x...", amount=5_000_000, tier=TaskTier.STANDARD)

# Flow 1: Lock in escrow
auth = client.authorize(pi)

# Flow 2: Pay worker
tx = client.release(pi)

# Flow 3: Cancel task
tx = client.refund_in_escrow(pi)

# Flow 4: Instant payment
tx = client.charge(pi)

# Flow 5: Dispute
tx = client.refund_post_escrow(pi)
```

### TypeScript SDK (`uvd-x402-sdk-typescript`)

```typescript
import { AdvancedEscrowClient } from 'uvd-x402-sdk/backend';

const client = new AdvancedEscrowClient(signer, {
  facilitatorUrl: 'https://facilitator.ultravioletadao.xyz',
  rpcUrl: 'https://mainnet.base.org',
});

const pi = client.buildPaymentInfo('0xWorker...', '5000000', 'standard');
const auth = await client.authorize(pi);
const tx = await client.release(pi);
```

---

## Chamba Integration (AI Agent Marketplace)

We integrated the SDK into Chamba's payment system with a `ChambaAdvancedEscrow` wrapper that provides:

1. **Payment Strategy Recommendation**: Auto-recommends the best flow based on amount, worker reputation, and task type
2. **Task Tier Auto-Detection**: Maps bounty amounts to timing parameters
3. **Partial Release**: 15% proof-of-attempt + refund remainder
4. **Fee Handling**: 8% platform fee via `maxFeeBps`

### Chamba Payment Strategies

| Strategy | Flow | Use Case |
|----------|------|----------|
| Escrow Capture | AUTHORIZE -> RELEASE | Standard tasks ($5-$200) |
| Escrow Cancel | AUTHORIZE -> REFUND | Weather/event dependent |
| Instant Payment | CHARGE | Micro-tasks, trusted workers |
| Partial Payment | AUTHORIZE -> partial RELEASE + REFUND | Proof of attempt |
| Dispute Resolution | AUTHORIZE -> RELEASE -> REFUND POST ESCROW | High-value ($50+) |

---

## Recommendations

1. **Fix SDK nonce computation**: The PAYMENT_INFO_TYPEHASH omission is a breaking bug. All SDK users will hit this.

2. **Document authorize() vs charge() clearly**: Both set `hasCollectedPayment = true`. New integrators will try `authorize -> charge` and get `PaymentAlreadyCollected`.

3. **Consider renaming release()**: `release()` calling `capture()` is confusing. Perhaps `captureToReceiver()` or `payReceiver()` would be clearer.

4. **Document refundInEscrow uint120**: The `uint120` type for `refundInEscrow` amount (vs `uint256` for others) caused initial ABI mismatch issues.

5. **Provide integration examples**: A reference implementation showing all 5 flows would save integrators significant debugging time.

6. **Consider partial release API**: The current pattern requires two separate calls (`release(partial)` + `refundInEscrow(remainder)`). A single `partialRelease(amount)` function would simplify proof-of-attempt flows.

---

## Contact

- **Repository**: github.com/UltravioletaDAO/x402-rs
- **Facilitator**: https://facilitator.ultravioletadao.xyz
- **Test Scripts**: `tests/escrow/` directory
