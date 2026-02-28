# Receipt-Gated Release — Development Plan

**Source:** Ali / BackTrackCo proposal (DOWN_DETECTOR document)
**Date:** 2026-02-27
**Status:** Future — waiting on upstream PR #935 (offer-receipt extension)
**Depends on:** x402r-contracts, SDK v2, PR #935 merge

---

## Summary

Client pays merchant via x402. Merchant handler returns 500. Client paid but got nothing.

Solution: merchant must prove delivery via a signed receipt. No receipt after a time window = automatic refund. Inverts the burden of proof — merchant proves delivery, not client proves non-delivery.

---

## Prerequisites (Must Be True Before Starting)

| # | Prerequisite | Status | How to Check |
|---|-------------|--------|--------------|
| 1 | PR #935 (offer-receipt extension) merged in upstream x402 | Pending | Check github.com/x402-rs/x402-rs |
| 2 | Receipt format finalized (EIP-712 struct fields) | Pending | Follows from PR #935 |
| 3 | x402r escrow stable in production (no open bugs) | Done (v1.34.2) | Test settle + verify with BackTrack |
| 4 | Facilitator operator-agnostic mode validated by BackTrack | Done (v1.34.2) | Ali confirms no issues |
| 5 | Base Sepolia test environment available | Done | Already in /supported |

---

## Architecture Overview

```
Client --> Merchant --> Facilitator (verify/settle with escrow)
                |
                +-- handler runs
                |
                +-- if 200: merchant signs receipt
                |            --> published on-chain (Tier 1)
                |            --> or sent to arbiter (Tier 2)
                |            --> release() becomes callable
                |
                +-- if 500: no receipt
                             --> window passes (10 min default)
                             --> anyone calls refundInEscrow()
                             --> payer gets money back
```

Two tiers:
- **Tier 1**: 3 Solidity contracts, fully trustless, on-chain receipt verification
- **Tier 2**: Off-chain arbiter service, supports JWT + EIP-712, enables content verification

---

## Phase 1 — Tier 1 Contracts (Trustless L0)

**Effort:** 3-4 days
**Scope:** Smart contracts only. No facilitator changes.

### Contracts to Deploy

| Contract | ~Lines | Purpose |
|----------|--------|---------|
| `ReceiptRegistry` | 80 | Store + verify EIP-712 signed receipts. Keyed by `paymentInfoHash`. |
| `ReceiptRequired` | 20 | ICondition for release: `registry.hasReceipt(hash)` |
| `NoReceiptRefund` | 30 | ICondition for refund: no receipt + window passed |

### ReceiptRegistry Key Design

```solidity
// EIP-712 struct for merchant-signed receipt
struct Receipt {
    bytes32 paymentInfoHash;   // links to escrow payment
    string  resourceUrl;       // what was delivered
    uint256 issuedAt;          // when
    bytes32 contentHash;       // optional: keccak256(responseBody) for L1+
}

// Core functions
function publishReceipt(paymentInfo, resourceUrl, issuedAt, signature) external;
function hasReceipt(bytes32 paymentInfoHash) external view returns (bool);
```

### Operator Configuration

```solidity
// Release: receipt exists OR payer approves manually
ICondition releaseCondition = new OrCondition([
    address(receiptRequired),       // receipt on-chain -> anyone can release
    address(new PayerCondition())   // payer happy -> payer releases directly
]);

// Refund: merchant voluntary OR no receipt after window
ICondition refundCondition = new OrCondition([
    address(new ReceiverCondition()),  // merchant voluntary refund
    address(noReceiptRefund)           // anyone after window + no receipt
]);
```

### Tasks

1. Write `ReceiptRegistry.sol` with EIP-712 verification
2. Write `ReceiptRequired.sol` (ICondition, ~20 lines)
3. Write `NoReceiptRefund.sol` (ICondition, reads EscrowPeriod for auth timestamp)
4. Write unit tests (Foundry)
5. Deploy to Base Sepolia
6. Configure a test operator with receipt-gated conditions
7. End-to-end test: authorize -> receipt -> release (happy path)
8. End-to-end test: authorize -> no receipt -> window passes -> refund (failure path)

### Validation Criteria

- [ ] Happy path: merchant signs receipt, release succeeds immediately
- [ ] Failure path: no receipt, refund available after 10 min window
- [ ] Replay protection: same receipt can't be published twice
- [ ] Signer check: only receiver address can sign valid receipts
- [ ] Gas cost per receipt < $0.05 on Base

---

## Phase 2 — Keeper Bot

**Effort:** 0.5 days
**Scope:** Simple bot that watches for settlements without receipts and triggers refunds.

### Design

- Watch `AuthCaptureEscrow` events for new authorizations
- After `RECEIPT_WINDOW` (10 min), check `ReceiptRegistry.hasReceipt(hash)`
- If no receipt: call `refundInEscrow()` on behalf of payer
- Bot needs gas but no special permissions (refund is permissionless)

### Open Question: Who Pays Gas?

For v1, the payer or a trusted service calls refund. No keeper bounty mechanism exists in current contracts. Options for v2:
- A: Facilitator itself runs the keeper (we pay gas, cost ~$0.001/refund on Base)
- B: Dedicated keeper with bounty (requires contract changes to split refund)
- C: BackTrack runs their own keeper

**Recommendation:** Option A for v1. Gas cost is negligible on L2s.

### Tasks

1. Write keeper script (TypeScript or Rust)
2. Watch settlement events from operator contract
3. Timer: wait RECEIPT_WINDOW after each authorization
4. Check receipt existence, call refund if missing
5. Deploy as lightweight service (Lambda or sidecar)

---

## Phase 3 — Merchant SDK Integration

**Effort:** 1-2 days
**Depends on:** PR #935 finalized

### What Merchants Need to Do

After their handler succeeds (200), sign a receipt:

```typescript
// In merchant's response handler
if (response.status === 200) {
    const receipt = await walletClient.signTypedData({
        domain: { name: "x402r-ReceiptRegistry", version: "1", chainId },
        types: { Receipt: [
            { name: "paymentInfoHash", type: "bytes32" },
            { name: "resourceUrl", type: "string" },
            { name: "issuedAt", type: "uint256" },
        ]},
        primaryType: "Receipt",
        message: { paymentInfoHash, resourceUrl, issuedAt: now() },
    });
    // Publish on-chain (or have a relayer do it)
    await receiptRegistry.publishReceipt(paymentInfo, resourceUrl, issuedAt, receipt);
}
```

### Tasks

1. Define receipt EIP-712 domain and types (must match contract)
2. SDK helper: `signReceipt(paymentInfo, resourceUrl)`
3. SDK helper: `publishReceipt(receipt)` (on-chain tx)
4. Document merchant integration (3-step guide)
5. Example: modify BackTrack's handler to sign receipts

---

## Phase 4 — Tier 2 Arbiter Service (Optional, L1+ Capable)

**Effort:** 2-3 days (L0) + 2-3 days (L1+)
**Scope:** Off-chain service for merchants who don't want to manage Ethereum keys.

### Why Tier 2

- Web2 merchants can use JWT instead of EIP-712
- Enables content verification (schema check, AI review)
- Arbiter publishes attestation on-chain instead of merchant publishing receipt

### Components

| Component | ~Lines | Purpose |
|-----------|--------|---------|
| `POST /verify` endpoint | 80 TS | Receive receipt, verify signature, publish attestation |
| Chain watcher | 80 TS | Fallback: detect settlements without attestations |
| Identity resolver | 50 TS | Resolve did:web for JWT merchants |
| ClaimRegistry contract | 100 Sol | Generalized on-chain claim storage |

### Merchant Identity

Uses `did:web` standard (from PR #935):
- Merchant hosts `/.well-known/did.json` with their public key
- Arbiter fetches key at verification time
- No pre-registration needed

### Trust Model

The arbiter CAN: fail to publish (payer gets refund), delay (bounded by window).
The arbiter CANNOT: forge receipts, steal funds, block refunds.

### Tasks

1. Write `ClaimRegistry.sol` (generalized ReceiptRegistry)
2. Write arbiter service skeleton (Express/Fastify)
3. Implement EIP-712 receipt verification path
4. Implement JWT receipt verification path with did:web resolver
5. Implement chain watcher for fallback refund detection
6. Deploy arbiter to AWS (Lambda or Fargate Spot)
7. End-to-end test with JWT merchant
8. End-to-end test with EIP-712 merchant via Tier 2

---

## Phase 5 — L1+ Content Verification (Future)

**Effort:** 3-5 days beyond Phase 4
**Scope:** Arbiter inspects actual response content.

### Verification Levels

| Level | Name | What It Checks | Signer |
|-------|------|---------------|--------|
| L0 | Delivery Protection | "did anything arrive?" | Merchant |
| L1 | Schema Check | "matches expected format?" | Rule engine |
| L2 | AI Review | "content is what was promised?" | AI verifier |
| L3 | Human Arbitration | "who is right?" | Human arbitrator |

### Key Addition: contentHash

Merchant includes `contentHash = keccak256(responseBody)` in receipt. Arbiter verifies hash matches actual payload, then runs L1+ checks.

### Backup Delivery

Arbiter stores payload during L1+ verification. If merchant withholds response, client fetches from arbiter via `GET /payload/:paymentInfoHash`.

### Not Planning Yet

- EigenLayer AVS integration (Tier 2 hardening)
- Lit Protocol encrypted payload availability
- On-chain JWS verification (RIP-7212)
- Keeper bounty mechanism

---

## Risk Assessment

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| PR #935 format changes after we build | Rework contracts | Medium | Wait for merge before Phase 1 |
| Receipt gas cost > payment value (micropayments) | Economic unviability for small payments | Medium | Tier 2 (off-chain) for micropayments |
| No one triggers refund (keeper offline) | Payer stuck until authorizationExpiry | Low | Multiple keeper instances, facilitator as fallback |
| Merchant signs receipt but delivers garbage | Client gets bad content | Medium | L1+ verification (Phase 5) |
| Arbiter goes offline (Tier 2) | Merchant can't release, payer gets refund | Low | Multiple arbiters, Tier 1 fallback |

---

## Estimated Timeline

| Phase | Effort | Can Start When |
|-------|--------|---------------|
| Phase 1 (Contracts) | 3-4 days | PR #935 merged |
| Phase 2 (Keeper) | 0.5 day | Phase 1 deployed to testnet |
| Phase 3 (Merchant SDK) | 1-2 days | Phase 1 + PR #935 receipt format stable |
| Phase 4 (Tier 2 Arbiter) | 2-3 days | Phase 1 validated |
| Phase 5 (L1+ Content) | 3-5 days | Phase 4 deployed |

**Total L0 (Phases 1-3):** ~1 week
**Total L0 + Tier 2 (Phases 1-4):** ~1.5 weeks
**Total with L1+ (all phases):** ~2.5 weeks

---

## Comparison with x402 Encrypted (Autoincentive)

| | x402 Encrypted | Receipt-Gated (This) |
|--|---------------|---------------------|
| Strategy | Verify before pay | Pay, auto-refund if no delivery |
| Trust gap | Moved (key delivery step) | Eliminated (escrow holds funds) |
| Merchant work | Encrypt every response | Sign receipt on 200 |
| Refund mechanism | None | On-chain, automatic |
| Works with existing x402 APIs | No | Yes |

These approaches are complementary, not competing. x402 Encrypted is L-1 (pre-payment confidence), receipt-gating is L0 (post-payment protection).

---

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-02-27 | Status: Future (not immediate) | Wait for PR #935, stabilize current escrow first |
| 2026-02-27 | Facilitator runs keeper (Phase 2) | Gas cost negligible on L2, simplest option |
| 2026-02-27 | Tier 1 first, Tier 2 optional | Trustless > minimal trust, build incrementally |
| 2026-02-27 | 10 min default receipt window | Covers slow handlers + Base congestion |
