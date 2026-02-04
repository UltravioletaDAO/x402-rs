# x402r Escrow Scheme - Complete Documentation

## Overview

The escrow scheme (`scheme: "escrow"`) is an advanced payment system that places funds in escrow before final settlement. This enables:

- **Authorize/Capture pattern**: Funds are held in escrow, then captured or released later
- **Refunds**: Both in-escrow (before capture) and post-escrow (after capture)
- **Conditions**: Pluggable logic for who can perform actions
- **Fees**: Protocol and operator fees on captured payments

## Architecture

```
┌─────────────┐     ┌─────────────────┐     ┌──────────────────────┐
│   Client    │────▶│   Facilitator   │────▶│  PaymentOperator     │
│  (Payer)    │     │  (x402-rs)      │     │  (Smart Contract)    │
└─────────────┘     └─────────────────┘     └──────────────────────┘
      │                     │                         │
      │  1. Sign ERC-3009   │                         │
      │  authorization      │                         │
      │                     │  2. Call authorize()    │
      │                     │─────────────────────────▶
      │                     │                         │
      │                     │                    ┌────▼────┐
      │                     │                    │ Escrow  │
      │                     │                    │ (holds  │
      │                     │                    │  USDC)  │
      │                     │                    └─────────┘
```

## Payment Flow

### Phase 1: Authorization (Facilitator handles this)

1. **Client** creates `EscrowPayload` with:
   - ERC-3009 `TransferWithAuthorization` data
   - Signature from payer
   - PaymentInfo for escrow parameters

2. **Client** sends to facilitator's `/settle` endpoint with `scheme: "escrow"`

3. **Facilitator** calls `PaymentOperator.authorize()`:
   - Tokens transferred from payer to escrow via ERC-3009
   - Payment recorded in `AuthCaptureEscrow` contract

### Phase 2: Capture/Release (Resource server handles this)

After service is delivered, the resource server can:
- **Release**: Capture funds and send to receiver
- **RefundInEscrow**: Return funds to payer (before capture)

### Phase 3: Post-Capture Refund (If needed)

After capture, within the refund window:
- **RefundPostEscrow**: Return captured funds to payer

---

## Contract Addresses

### Base Mainnet (eip155:8453)

| Contract | Address | Description |
|----------|---------|-------------|
| AuthCaptureEscrow | `0x320a3c35F131E5D2Fb36af56345726B298936037` | Core escrow logic |
| PaymentOperatorFactory | `0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838` | Deploys PaymentOperator instances |
| TokenCollector | `0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6` | Receives ERC-3009 transfers |
| ProtocolFeeConfig | `0x230fd3A171750FA45db2976121376b7F47Cba308` | Protocol fee settings |
| RefundRequest | `0xc1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98` | Refund request management |
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | USDC token contract |

### Base Sepolia (eip155:84532)

| Contract | Address | Description |
|----------|---------|-------------|
| AuthCaptureEscrow | `0xb9488351E48b23D798f24e8174514F28B741Eb4f` | Core escrow logic |
| PaymentOperatorFactory | `0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70` | Deploys PaymentOperator instances |
| TokenCollector | `0xC80cd08d609673061597DE7fe54Af3978f10A825` | Receives ERC-3009 transfers |
| ProtocolFeeConfig | `0x1e52a74cE6b69F04a506eF815743E1052A1BD28F` | Protocol fee settings |
| RefundRequest | `0x6926c05193c714ED4bA3867Ee93d6816Fdc14128` | Refund request management |
| USDC | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` | USDC token contract |

---

## Data Structures

### EscrowPayload (sent to facilitator)

```json
{
  "authorization": {
    "from": "0xPAYER_ADDRESS",      // Payer's address (signer)
    "to": "0xTOKEN_COLLECTOR",      // TokenCollector contract
    "value": "1000000",             // Amount in token decimals (1 USDC = 1000000)
    "validAfter": "0",              // Unix timestamp (seconds)
    "validBefore": "1738500000",    // Unix timestamp (seconds)
    "nonce": "0x..."                // 32-byte nonce (hash of paymentInfo)
  },
  "signature": "0x...",             // 65-byte ERC-3009 signature
  "paymentInfo": {
    "operator": "0xOPERATOR",       // PaymentOperator address
    "receiver": "0xRECEIVER",       // Who receives the payment
    "token": "0xUSDC",              // Token contract
    "maxAmount": "1000000",         // Max authorized amount
    "preApprovalExpiry": 281474976710655,    // uint48 max = never expires
    "authorizationExpiry": 281474976710655,  // uint48 max = never expires
    "refundExpiry": 281474976710655,         // uint48 max = never expires
    "minFeeBps": 0,                 // Minimum fee in basis points
    "maxFeeBps": 100,               // Maximum fee (100 = 1%)
    "feeReceiver": "0xFEE_RECV",    // Who receives fees
    "salt": "0x..."                 // Random 32 bytes for uniqueness
  }
}
```

### PaymentRequirements.extra (configuration)

```json
{
  "escrowAddress": "0x...",      // AuthCaptureEscrow contract
  "operatorAddress": "0x...",   // PaymentOperator address
  "tokenCollector": "0x...",    // TokenCollector for ERC-3009
  "name": "USD Coin",           // EIP-712 domain name (optional)
  "version": "2"                // EIP-712 domain version (optional)
}
```

---

## Nonce Computation

The nonce is critical for security. It's computed as:

```
nonce = keccak256(abi.encode(chainId, escrowAddress, paymentInfoWithZeroPayer))
```

Where `paymentInfoWithZeroPayer` has `payer = address(0)` to make the nonce payer-agnostic.

### Why payer-agnostic?

This allows the same paymentInfo to be used by any payer. The actual payer is determined by who signs the authorization.

---

## ERC-3009 Signature

The signature authorizes a token transfer using EIP-712 typed data:

### Domain

```javascript
{
  name: "USD Coin",           // Token name (varies by chain!)
  version: "2",               // Token version
  chainId: 8453,              // Base mainnet
  verifyingContract: "0x..."  // USDC contract address
}
```

**WARNING**: Domain name varies by chain:
- Base: `"USD Coin"` or `"USDC"`
- Ethereum: `"USD Coin"`
- Check token contract's `name()` function

### Message Type

```javascript
{
  TransferWithAuthorization: [
    { name: "from", type: "address" },
    { name: "to", type: "address" },
    { name: "value", type: "uint256" },
    { name: "validAfter", type: "uint256" },
    { name: "validBefore", type: "uint256" },
    { name: "nonce", type: "bytes32" }
  ]
}
```

---

## Expiry Timestamps

All timestamps are **Unix seconds** (not milliseconds!).

| Field | Type | Description |
|-------|------|-------------|
| `validAfter` | uint256 | Authorization valid after this time (usually 0) |
| `validBefore` | uint256 | Authorization expires at this time |
| `preApprovalExpiry` | uint48 | Pre-approval window (use MAX_UINT48 for no expiry) |
| `authorizationExpiry` | uint48 | How long authorization can be captured |
| `refundExpiry` | uint48 | How long after capture refunds are allowed |

### Constants

```
MAX_UINT48 = 281474976710655  // Use for "never expires"
```

---

## Fee System

Fees are charged when payments are captured (not on authorize).

| Parameter | Type | Description |
|-----------|------|-------------|
| `minFeeBps` | uint16 | Minimum fee in basis points |
| `maxFeeBps` | uint16 | Maximum fee in basis points |
| `feeReceiver` | address | Who receives the fee |

**Basis Points**: 100 bps = 1%, 10000 bps = 100%

Example: `maxFeeBps: 100` = 1% maximum fee

---

## Full Request Example

```json
{
  "x402Version": 2,
  "scheme": "escrow",
  "payload": {
    "authorization": {
      "from": "0x1234567890123456789012345678901234567890",
      "to": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6",
      "value": "1000000",
      "validAfter": "0",
      "validBefore": "1738500000",
      "nonce": "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
    },
    "signature": "0x...(65 bytes hex)...",
    "paymentInfo": {
      "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
      "receiver": "0xRECEIVER_ADDRESS",
      "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "maxAmount": "1000000",
      "preApprovalExpiry": 281474976710655,
      "authorizationExpiry": 281474976710655,
      "refundExpiry": 281474976710655,
      "minFeeBps": 0,
      "maxFeeBps": 100,
      "feeReceiver": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
      "salt": "0x0000000000000000000000000000000000000000000000000000000000000001"
    }
  },
  "paymentRequirements": {
    "scheme": "escrow",
    "network": "eip155:8453",
    "maxAmountRequired": "1000000",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "payTo": "0xRECEIVER_ADDRESS",
    "extra": {
      "escrowAddress": "0x320a3c35F131E5D2Fb36af56345726B298936037",
      "operatorAddress": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
      "tokenCollector": "0x32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6"
    }
  }
}
```

---

## Error Cases

| Error | Cause | Solution |
|-------|-------|----------|
| `Invalid ERC-3009 signature` | Wrong signer or domain | Check domain name/version, signer address |
| `Escrow scheme is disabled` | Feature flag off | Set `ENABLE_PAYMENT_OPERATOR=true` |
| `Network not supported` | Wrong network | Use Base mainnet or Base Sepolia |
| `Insufficient payment amount` | value < maxAmountRequired | Increase authorization value |
| `Token mismatch` | paymentInfo.token != requirements.asset | Use correct USDC address |
| `Authorization expired` | validBefore < current time | Use future timestamp |

---

## Testing Checklist

- [ ] Basic authorize flow (happy path)
- [ ] Different amounts (1 USDC, 0.01 USDC, 100 USDC)
- [ ] Expiry timestamps (valid, near-expiry, expired)
- [ ] Invalid signature (wrong signer, wrong domain)
- [ ] Insufficient balance
- [ ] Wrong network
- [ ] Wrong token address
- [ ] Invalid nonce
- [ ] Base Sepolia (testnet)
- [ ] Base Mainnet (production)

---

## Test Results

*This section will be updated as tests are run.*

### Test Run: [DATE]

| Test | Network | Amount | Result | Notes |
|------|---------|--------|--------|-------|
| TBD | | | | |

---

## References

- [x402r-scheme reference implementation](https://github.com/BackTrackCo/x402r-scheme)
- [x402r-sdk configuration](https://github.com/BackTrackCo/x402r-sdk/blob/main/packages/core/src/config/index.ts)
- [ERC-3009 specification](https://eips.ethereum.org/EIPS/eip-3009)
- [EIP-712 typed data signing](https://eips.ethereum.org/EIPS/eip-712)
