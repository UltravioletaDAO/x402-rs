# x402r Escrow Integration - Bug Report & Questions

Hi Ali,

We've been integrating the x402r escrow scheme into our facilitator and found a critical bug in the SDK plus an architecture question.

---

## Bug: SDK Nonce Computation is Wrong

**The `computeEscrowNonce()` function in `x402r-scheme/packages/evm/src/shared/nonce.ts` doesn't match the on-chain contract.**

We verified by calling `AuthCaptureEscrow.getHash()` on Base mainnet:

```
On-chain getHash(): 0x8278d8424034803841e39468fa9458fc21006bd8c90078d9c023fd2905347a9e
SDK nonce:          0xc8f0cfb3669d0f6d9c7228637109267645a55a9de4ba6d401b2d474748a6872e
```

**Root cause**: The contract uses `PAYMENT_INFO_TYPEHASH` in a two-step hash, but the SDK doesn't:

```solidity
// Contract (AuthCaptureEscrow.sol:421-424)
function getHash(PaymentInfo calldata paymentInfo) public view returns (bytes32) {
    bytes32 paymentInfoHash = keccak256(abi.encode(PAYMENT_INFO_TYPEHASH, paymentInfo));
    return keccak256(abi.encode(block.chainid, address(this), paymentInfoHash));
}
```

```typescript
// SDK (nonce.ts) - MISSING the TYPEHASH step!
const encoded = encodeAbiParameters([chainId, escrow, paymentInfo], [...]);
return keccak256(encoded);
```

---

## Question: How should facilitators call authorize()?

`AuthCaptureEscrow.authorize()` has `onlySender(paymentInfo.operator)`, meaning `msg.sender` must equal `paymentInfo.operator`.

**Options we see:**
1. Set `paymentInfo.operator` to the facilitator wallet address
2. Deploy a PaymentOperator where the facilitator is the owner

**Which is the intended approach?** Is there a reference facilitator implementation we can look at?

---

## What's Working

- ERC-3009 signatures work (verified with direct USDC calls)
- Contract addresses from x402r-sdk are correct
- Our ABI encoding matches the contract

---

Let me know if you need more details or test scripts!
