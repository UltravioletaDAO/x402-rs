# Receipt-Gated Release — Implementation Plan

**Source:** Ali / BackTrackCo proposal (DOWN_DETECTOR document)
**Date:** 2026-02-27 (original), 2026-03-13 (updated for PR #935 merge)
**Status:** READY TO BUILD
**Depends on:** x402r-contracts (done), SDK v2 (done), PR #935 merge (DONE 2026-03-12)

---

## Summary

Client pays merchant via x402. Merchant handler returns 500. Client paid but got nothing.

Solution: merchant must prove delivery via a signed receipt. No receipt after a time window = automatic refund. Inverts the burden of proof — merchant proves delivery, not client proves non-delivery.

---

## Prerequisites (All Met)

| # | Prerequisite | Status | Notes |
|---|-------------|--------|-------|
| 1 | PR #935 (offer-receipt extension) merged | DONE (2026-03-12) | coinbase/x402 PR #935 merged |
| 2 | Receipt format finalized | DONE | EIP-712 + JWS, domain `{name:"x402 receipt", version:"1", chainId:1}` |
| 3 | x402r escrow stable in production | DONE (v1.34.2) | Validated with BackTrack |
| 4 | Facilitator operator-agnostic mode validated | DONE (v1.34.2) | Ali confirms no issues |
| 5 | Base Sepolia test environment available | DONE | Already in /supported |

---

## PR #935 Receipt Format (Canonical Reference)

PR #935 adds `extensions["offer-receipt"]` to x402 settlement responses. Key details:

**Receipt payload fields:**
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `version` | number | Yes | Schema version (currently 1) |
| `network` | string | Yes | Always CAIP-2 (e.g., "eip155:8453") |
| `resourceUrl` | string | Yes | The paid resource URL |
| `payer` | string | Yes | Payer wallet address |
| `issuedAt` | number | Yes | Unix timestamp |
| `transaction` | string | Optional | Tx hash (omitted by default for privacy) |

**EIP-712 domain:** `{ name: "x402 receipt", version: "1", chainId: 1 }` (chainId hardcoded to 1 regardless of payment network)

**JWS format:** Uses did:web for key resolution. Merchant hosts `/.well-known/did.json`.

**Wire format:**
```json
{
  "extensions": {
    "offer-receipt": {
      "info": {
        "receipt": {
          "format": "eip712",
          "payload": { "version": 1, "network": "eip155:8453", "resourceUrl": "/api/data", "payer": "0x...", "issuedAt": 1710000000 },
          "signature": "0x..."
        }
      },
      "schema": { ... }
    }
  }
}
```

**Format alignment with our escrow system:**
- PR #935 receipts don't have `paymentInfoHash` — they use `payer + network + resourceUrl` to identify payments
- Our escrow uses `paymentInfoHash` (keccak256 of full PaymentInfo struct) to link payments
- Bridge: `ReceiptRegistry` contract accepts both the PR #935 receipt fields AND the `paymentInfoHash` for escrow linkage
- The contract hashes the PR #935 fields internally to verify the EIP-712 signature, then stores against `paymentInfoHash`

---

## Architecture Overview

```
Client --> Merchant --> Facilitator (verify/settle with escrow)
                |
                +-- handler runs
                |
                +-- if 200: merchant signs receipt (PR #935 format)
                |            --> facilitator registers on-chain (ReceiptRegistry)
                |            --> ReceiptRequired.check() returns true
                |            --> release() becomes callable immediately
                |
                +-- if 500: no receipt
                             --> receipt window passes (10 min default)
                             --> keeper calls refundInEscrow()
                             --> NoReceiptRefund.check() returns true
                             --> payer gets money back
```

Key insight: the **facilitator** registers the receipt on-chain, not the merchant. The merchant signs the receipt (PR #935 extension) and sends it in the settlement response. The facilitator verifies the signature and calls `ReceiptRegistry.registerReceipt()`. This keeps the merchant experience simple — just enable the offer-receipt extension.

---

## Phase 1 — Tier 1 Contracts (Trustless L0)

**Effort:** 3-4 days
**Scope:** Smart contracts only. Deploy to Base Sepolia, then Base Mainnet.

### 1.1 ReceiptRegistry.sol (~120 lines)

Stores merchant-signed receipts, verified via EIP-712 ecrecover.

```solidity
contract ReceiptRegistry is EIP712 {
    using ECDSA for bytes32;

    // EIP-712 typehash matching PR #935 receipt format
    bytes32 public constant RECEIPT_TYPEHASH = keccak256(
        "Receipt(uint8 version,string network,string resourceUrl,address payer,uint256 issuedAt)"
    );

    struct StoredReceipt {
        uint8   version;
        string  network;         // CAIP-2
        string  resourceUrl;
        address payer;
        uint256 issuedAt;
        address signer;          // recovered from signature (must == paymentInfo.receiver)
        uint256 publishedAt;     // block.timestamp when registered
    }

    // paymentInfoHash -> StoredReceipt
    mapping(bytes32 => StoredReceipt) public receipts;

    event ReceiptRegistered(
        bytes32 indexed paymentInfoHash,
        address indexed merchant,
        address payer,
        string network,
        uint256 issuedAt
    );

    error ReceiptAlreadyExists();
    error SignerNotReceiver();

    // Domain: { name: "x402 receipt", version: "1" } — matches PR #935
    constructor() EIP712("x402 receipt", "1") {}

    function registerReceipt(
        AuthCaptureEscrow.PaymentInfo calldata paymentInfo,
        uint8 version,
        string calldata network,
        string calldata resourceUrl,
        address payer,
        uint256 issuedAt,
        bytes calldata signature
    ) external {
        bytes32 paymentInfoHash = _hashPaymentInfo(paymentInfo);
        if (receipts[paymentInfoHash].publishedAt != 0) revert ReceiptAlreadyExists();

        // Verify EIP-712 signature recovers to paymentInfo.receiver (merchant)
        bytes32 structHash = keccak256(abi.encode(
            RECEIPT_TYPEHASH,
            version,
            keccak256(bytes(network)),
            keccak256(bytes(resourceUrl)),
            payer,
            issuedAt
        ));
        address signer = _hashTypedDataV4(structHash).recover(signature);
        if (signer != paymentInfo.receiver) revert SignerNotReceiver();

        receipts[paymentInfoHash] = StoredReceipt({
            version: version,
            network: network,
            resourceUrl: resourceUrl,
            payer: payer,
            issuedAt: issuedAt,
            signer: signer,
            publishedAt: block.timestamp
        });

        emit ReceiptRegistered(paymentInfoHash, signer, payer, network, issuedAt);
    }

    function hasReceipt(bytes32 paymentInfoHash) external view returns (bool) {
        return receipts[paymentInfoHash].publishedAt != 0;
    }
}
```

**NOTE on EIP-712 chainId:** PR #935 hardcodes `chainId: 1` in the domain separator. This is intentional — receipts are chain-agnostic proofs. The `paymentInfoHash` provides chain-specific binding to the escrow. Our contract's `EIP712("x402 receipt", "1")` constructor will use the deployment chain's ID by default. We need to override `_domainSeparatorV4()` to force `chainId: 1`, or use a custom domain separator computation. This is a critical implementation detail to verify.

### 1.2 ReceiptRequired.sol (~25 lines)

Implements `ICondition` — release gate.

```solidity
contract ReceiptRequired is ICondition {
    ReceiptRegistry public immutable REGISTRY;
    AuthCaptureEscrow public immutable ESCROW;

    constructor(ReceiptRegistry registry, AuthCaptureEscrow escrow) {
        REGISTRY = registry;
        ESCROW = escrow;
    }

    function check(
        AuthCaptureEscrow.PaymentInfo calldata paymentInfo,
        uint256,        // amount (unused)
        address         // caller (unused)
    ) external view returns (bool) {
        return REGISTRY.hasReceipt(ESCROW.getHash(paymentInfo));
    }
}
```

### 1.3 NoReceiptRefund.sol (~40 lines)

Implements `ICondition` — refund gate after receipt window passes.

```solidity
contract NoReceiptRefund is ICondition {
    ReceiptRegistry public immutable REGISTRY;
    AuthCaptureEscrow public immutable ESCROW;
    uint256 public immutable RECEIPT_WINDOW;  // seconds (default: 600 = 10 min)

    constructor(ReceiptRegistry registry, AuthCaptureEscrow escrow, uint256 receiptWindow) {
        REGISTRY = registry;
        ESCROW = escrow;
        RECEIPT_WINDOW = receiptWindow;
    }

    function check(
        AuthCaptureEscrow.PaymentInfo calldata paymentInfo,
        uint256,        // amount (unused)
        address         // caller (unused)
    ) external view returns (bool) {
        bytes32 hash = ESCROW.getHash(paymentInfo);
        // Can't refund if receipt exists
        if (REGISTRY.hasReceipt(hash)) return false;
        // Need to check if receipt window has passed since authorization
        // Uses escrow's payment state to get authorization timestamp
        // If block.timestamp > authTime + RECEIPT_WINDOW, refund is allowed
        // Implementation depends on how AuthCaptureEscrow exposes auth timestamp
        return true; // simplified — real impl checks timestamp
    }
}
```

### 1.4 Operator Configuration

When deploying a new PaymentOperator for receipt-gated escrow:

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

### 1.5 Deployment Sequence

1. Deploy `ReceiptRegistry` on Base Sepolia
2. Deploy `ReceiptRequired(registry, escrow)` on Base Sepolia
3. Deploy `NoReceiptRefund(registry, escrow, 600)` on Base Sepolia (10 min window)
4. Deploy a test PaymentOperator via factory with receipt-gated conditions
5. Run test suite (happy path + failure path)
6. Repeat 1-4 on Base Mainnet
7. Add addresses to `src/payment_operator/addresses.rs`
8. Generate ABI JSONs → `abi/ReceiptRegistry.json`

### 1.6 Validation Criteria

- [ ] Happy path: merchant signs receipt, facilitator registers, release succeeds immediately
- [ ] Failure path: no receipt, refund available after 10 min window
- [ ] Replay protection: same receipt can't be registered twice (ReceiptAlreadyExists)
- [ ] Signer check: only receiver address can sign valid receipts (SignerNotReceiver)
- [ ] Gas cost per registerReceipt() < $0.05 on Base Mainnet
- [ ] EIP-712 domain matches PR #935 (`name: "x402 receipt"`, `version: "1"`, `chainId: 1`)

---

## Phase 2 — Facilitator Rust Integration

**Effort:** 2-3 days
**Scope:** Rust code changes to support receipt extension pass-through and on-chain registration.

### 2.1 New module: `src/receipt.rs` (~200 lines)

```rust
// Receipt-gated release support — PR #935 offer-receipt extension

pub const EXTENSION_ID: &str = "offer-receipt";

/// PR #935 receipt payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptPayload {
    pub version: u8,
    pub network: String,        // CAIP-2 format
    #[serde(rename = "resourceUrl")]
    pub resource_url: String,
    pub payer: String,
    #[serde(rename = "issuedAt")]
    pub issued_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,
}

/// Signed receipt (EIP-712 or JWS)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedReceipt {
    pub format: String,         // "eip712" or "jws"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<ReceiptPayload>,  // present for eip712, absent for jws
    pub signature: String,
}

/// Parse offer-receipt extension from settlement response extensions
pub fn parse_receipt_extension(extensions: &serde_json::Value) -> Option<SignedReceipt> { ... }

/// Verify EIP-712 receipt signature
/// Domain: { name: "x402 receipt", version: "1", chainId: 1 }
pub fn verify_eip712_receipt(receipt: &ReceiptPayload, signature: &[u8]) -> Option<Address> { ... }

/// Register receipt on-chain via ReceiptRegistry contract
pub async fn register_receipt_onchain(
    provider: &Provider,
    registry_address: Address,
    payment_info: &ContractPaymentInfo,
    receipt: &ReceiptPayload,
    signature: &[u8],
    signer: &PrivateKeySigner,
) -> Result<TxHash> { ... }

/// Check if receipt exists on-chain
pub async fn has_receipt(
    provider: &Provider,
    registry_address: Address,
    payment_info_hash: B256,
) -> Result<bool> { ... }

/// Feature flag
pub fn is_receipt_gating_enabled() -> bool {
    std::env::var("ENABLE_RECEIPT_GATING")
        .map(|v| v == "true")
        .unwrap_or(false)
}
```

### 2.2 Add `extensions` field to SettleResponse

In `src/types.rs` (around line 1401), add optional extensions field:

```rust
pub struct SettleResponse {
    pub success: bool,
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<HashMap<String, serde_json::Value>>,
}
```

Backward-compatible — existing clients see no change.

### 2.3 Receipt generation in EVM settle flow

In `src/chain/evm.rs`, after successful settlement, if the payment used escrow with receipt-gated conditions:

1. Facilitator signs a receipt using EIP-712 (PR #935 format)
2. Calls `ReceiptRegistry.registerReceipt()` on-chain
3. Includes the signed receipt in `SettleResponse.extensions["offer-receipt"]`

This goes near the `create_proof_of_payment` call (~line 1036).

### 2.4 Receipt registration in PaymentOperator flow

In `src/payment_operator/operator.rs`, after escrow authorization succeeds:

1. Check if the operator uses `ReceiptRequired` as release condition
2. If yes, generate and register receipt after settlement
3. The receipt registration is a separate on-chain tx (can be batched later)

### 2.5 Contract addresses

Add to `src/payment_operator/addresses.rs`:

```rust
pub const RECEIPT_REGISTRY_BASE_MAINNET: &str = "0x...";  // after deployment
pub const RECEIPT_REGISTRY_BASE_SEPOLIA: &str = "0x...";
```

### 2.6 ABI binding

Add `abi/ReceiptRegistry.json` (generated from Solidity compilation).
Add Rust bindings via `sol!` macro or manual ABI encoding.

### 2.7 Environment variables

```
ENABLE_RECEIPT_GATING=true
```

Add to `.env.example` and Terraform task definition.

### 2.8 Files to modify

| File | Change | Lines est. |
|------|--------|-----------|
| `src/receipt.rs` | NEW — receipt types, verification, on-chain registration | ~200 |
| `src/main.rs` | Add `mod receipt;` | ~1 |
| `src/types.rs` | Add `extensions` field to `SettleResponse` | ~5 |
| `src/chain/evm.rs` | Receipt generation after settlement | ~60 |
| `src/payment_operator/operator.rs` | Receipt registration after escrow auth | ~80 |
| `src/payment_operator/addresses.rs` | Add ReceiptRegistry addresses | ~10 |
| `src/handlers.rs` | Optional: `/receipt/status` endpoint | ~40 |
| `src/openapi.rs` | Document new extension in OpenAPI | ~30 |
| `.env.example` | Add `ENABLE_RECEIPT_GATING` | ~3 |
| `abi/ReceiptRegistry.json` | NEW — contract ABI | Generated |

---

## Phase 3 — Keeper Bot

**Effort:** 0.5-1 day
**Scope:** Watches for settlements without receipts, triggers refunds.

### Design

```
loop every 5 minutes:
    1. Query AuthCaptureEscrow for recent authorizations (last 24h)
    2. For each authorization with ReceiptRequired condition:
       a. Check ReceiptRegistry.hasReceipt(paymentInfoHash)
       b. If no receipt AND authTime + RECEIPT_WINDOW < now:
          - Call refundInEscrow(paymentInfo) via PaymentOperator
          - Log refund event
    3. Track processed payments to avoid duplicate calls
```

### Implementation

`scripts/keeper_receipt_refund.py` (~200 lines) or `crates/keeper/` (Rust binary)

Python is faster to build; Rust is better for production reliability. Start with Python, migrate to Rust if needed.

### Deployment

- ECS Scheduled Task (every 5 minutes) — cheapest option
- Or Lambda with EventBridge schedule
- Needs: RPC access, facilitator private key (for gas), ReceiptRegistry + Escrow contract addresses

### Gas costs

- `refundInEscrow()` ~50k gas on Base (~$0.001)
- Expected volume: <10 refunds/day initially
- Monthly cost: negligible (<$1)

---

## Phase 4 — SDK Helpers

**Effort:** 1-2 days
**Scope:** TypeScript + Python SDK updates.

### TypeScript SDK (`uvd-x402-sdk-typescript`)

```typescript
// New exports from sdk
export function parseReceiptFromSettleResponse(response: SettleResponse): SignedReceipt | null;
export function verifyReceiptSignature(receipt: SignedReceipt): { valid: boolean; signer: string };

// Merchant-side helper (for merchants using our SDK)
export function createOfferReceiptExtension(signerKey: string): {
    enrichSettlementResponse: (context: SettleContext) => Promise<ExtensionData>;
};
```

### Python SDK (`uvd-x402-sdk-python`)

Mirror TypeScript helpers in Python.

### Merchant Integration Guide

For merchants using PR #935's `offer-receipt` extension:

1. Enable the extension in your x402 middleware config
2. Provide a signer (payTo private key for EIP-712, or any JWT key for JWS)
3. The facilitator handles the rest (on-chain registration, release gating)

```typescript
// Merchant server — minimal integration
import { createOfferReceiptExtension, createEIP712OfferReceiptIssuer } from "@x402/extensions";

const issuer = createEIP712OfferReceiptIssuer("did:web:merchant.example.com#key-1", signTypedData);
const offerReceipt = createOfferReceiptExtension(issuer);

app.get("/api/data", x402({
    extensions: [offerReceipt],
    // ... standard x402 config
}));
```

---

## Phase 5 — Tier 2 Arbiter Service (Future)

**Effort:** 2-3 days (L0) + 2-3 days (L1+)
**Scope:** Off-chain service for JWT merchants and content verification.
**Status:** Not immediate — build after Tier 1 is validated in production.

### Why Tier 2

- Web2 merchants can use JWT instead of EIP-712
- Enables content verification (L1+ — schema check, AI review)
- Arbiter publishes attestation on-chain instead of direct receipt registration

### Components

| Component | ~Lines | Purpose |
|-----------|--------|---------|
| `POST /verify` endpoint | 80 TS | Receive receipt, verify sig, publish attestation |
| Chain watcher | 80 TS | Fallback: detect settlements without attestations |
| Identity resolver | 50 TS | Resolve did:web for JWT merchants |
| ClaimRegistry contract | 100 Sol | Generalized on-chain claim storage |

### Merchant Identity (from PR #935)

Uses `did:web` standard:
- Merchant hosts `/.well-known/did.json` with their public key
- JWS header includes `kid` (DID URL) pointing to the verification method
- Arbiter fetches key at verification time — no pre-registration

### Trust Model

The arbiter CAN: fail to publish (payer gets refund), delay (bounded by window).
The arbiter CANNOT: forge receipts, steal funds, block refunds.

---

## Phase 6 — L1+ Content Verification (Future)

**Status:** Not planning yet. Depends on Phase 5.

### Verification Levels

| Level | Name | What It Checks | Signer |
|-------|------|---------------|--------|
| L0 | Delivery Protection | "did anything arrive?" | Merchant |
| L1 | Schema Check | "matches expected format?" | Rule engine |
| L2 | AI Review | "content is what was promised?" | AI verifier |
| L3 | Human Arbitration | "who is right?" | Human arbitrator |

### Key Addition: contentHash

Merchant includes `contentHash = keccak256(responseBody)` in receipt (not in PR #935 base spec, would be our extension). Arbiter verifies hash matches actual payload, then runs L1+ checks.

### Not Planning Yet

- EigenLayer AVS integration
- Lit Protocol encrypted payload availability
- On-chain JWS verification (RIP-7212)
- Keeper bounty mechanism

---

## Implementation Sequence (What to Build, In Order)

| Step | What | Where | Blocked By | Est. |
|------|------|-------|-----------|------|
| **1a** | ReceiptRegistry.sol | Solidity (new repo or contracts/) | Nothing | 1 day |
| **1b** | ReceiptRequired.sol | Solidity | 1a | 0.5 day |
| **1c** | NoReceiptRefund.sol | Solidity | 1a | 0.5 day |
| **1d** | Foundry unit tests | Solidity tests | 1a-c | 1 day |
| **1e** | Deploy Base Sepolia | scripts/ | 1d | 0.5 day |
| **2a** | `src/receipt.rs` module | x402-rs | 1e (for ABI) | 1 day |
| **2b** | `SettleResponse.extensions` field | src/types.rs | Nothing | 0.5 hr |
| **2c** | Receipt generation in evm.rs | src/chain/evm.rs | 2a | 0.5 day |
| **2d** | Receipt registration in operator.rs | src/payment_operator/ | 2a | 0.5 day |
| **2e** | Addresses + env vars | addresses.rs, .env | 1e | 0.5 hr |
| **2f** | OpenAPI docs | src/openapi.rs | 2c | 0.5 hr |
| **3a** | Keeper bot | scripts/ | 1e | 1 day |
| **4a** | TypeScript SDK helpers | uvd-x402-sdk-typescript | 2b | 0.5 day |
| **4b** | Python SDK helpers | uvd-x402-sdk-python | 2b | 0.5 day |
| **1f** | Deploy Base Mainnet | scripts/ | E2E tests pass | 0.5 day |

**Critical path:** 1a -> 1d -> 1e -> 2a -> 2c/2d -> E2E test -> 1f

**Total L0 (Phases 1-4):** ~8-10 working days
**Parallelizable:** SDK helpers (4a, 4b) can start after 2b. Keeper (3a) can start after 1e.

---

## Risk Assessment

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| EIP-712 chainId mismatch (PR #935 uses 1, Base is 8453) | Signature verification fails | High | Override domain separator in contract to force chainId:1 |
| PR #935 receipt format evolves post-merge | Rework signature verification | Low (just merged) | Contract stores against paymentInfoHash, format changes only affect off-chain verification |
| Receipt gas cost > payment value (micropayments) | Economic unviability | Medium | Tier 2 (off-chain) for micropayments |
| Keeper bot misses refund window | Payer stuck until authorizationExpiry | Low | Multiple keeper instances, facilitator as fallback |
| BackTrack doesn't implement offer-receipt extension | No one uses the feature | Medium | We provide SDK helpers, coordinate with Ali |

---

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-02-27 | Status: Future (not immediate) | Wait for PR #935, stabilize current escrow first |
| 2026-02-27 | Facilitator runs keeper (Phase 3) | Gas cost negligible on L2, simplest option |
| 2026-02-27 | Tier 1 first, Tier 2 optional | Trustless > minimal trust, build incrementally |
| 2026-02-27 | 10 min default receipt window | Covers slow handlers + Base congestion |
| 2026-03-13 | Status: READY TO BUILD | PR #935 merged, all prerequisites met |
| 2026-03-13 | Align receipt format with PR #935 | Use `{version, network, resourceUrl, payer, issuedAt}` not custom format |
| 2026-03-13 | Facilitator registers receipts, not merchant | Keeps merchant integration minimal — just enable extension |
| 2026-03-13 | EIP-712 domain `chainId:1` per PR #935 | Chain-agnostic receipts, paymentInfoHash provides chain binding |
| 2026-03-13 | Start with Base Sepolia, then Base Mainnet | Same pattern as existing escrow deployment |
