# Execution Market (EM) Integration Guide

## Overview

This guide documents how Execution Market integrates with the facilitator's gasless escrow lifecycle. The facilitator handles all on-chain transactions, so neither agents nor workers need gas tokens.

## Escrow Lifecycle

```
Agent creates task          Worker completes task       Agent approves
      |                           |                         |
      v                           v                         v
  authorize()              (off-chain work)            release()
  Lock $USDC               Worker does task         Send $USDC to worker
  in escrow                                          from escrow
      |                                                     |
      |--- Agent cancels task -----> refundInEscrow() ------|
                                     Return $USDC to agent
```

## Endpoints

### POST /settle (authorize)

Lock funds in escrow when a task is created.

```json
{
  "x402Version": 2,
  "scheme": "escrow",
  "action": "authorize",
  "payload": {
    "authorization": {
      "from": "0xAGENT_ADDRESS",
      "to": "0xTOKEN_COLLECTOR",
      "value": "1000000",
      "validAfter": "0",
      "validBefore": "1738500000",
      "nonce": "0x..."
    },
    "signature": "0x...",
    "paymentInfo": {
      "operator": "0xOPERATOR_ADDRESS",
      "receiver": "0xWORKER_ADDRESS",
      "token": "0xUSDC_ADDRESS",
      "maxAmount": "1000000",
      "preApprovalExpiry": 281474976710655,
      "authorizationExpiry": 281474976710655,
      "refundExpiry": 281474976710655,
      "minFeeBps": 0,
      "maxFeeBps": 100,
      "feeReceiver": "0xOPERATOR_ADDRESS",
      "salt": "0x..."
    }
  },
  "paymentRequirements": {
    "scheme": "escrow",
    "network": "eip155:8453",
    "extra": {
      "escrowAddress": "0xESCROW_ADDRESS",
      "operatorAddress": "0xOPERATOR_ADDRESS",
      "tokenCollector": "0xTOKEN_COLLECTOR"
    }
  }
}
```

### POST /settle (release)

Send escrowed funds to the worker when task is approved. No ERC-3009 signature needed.

```json
{
  "x402Version": 2,
  "scheme": "escrow",
  "action": "release",
  "payload": {
    "paymentInfo": {
      "operator": "0xOPERATOR_ADDRESS",
      "receiver": "0xWORKER_ADDRESS",
      "token": "0xUSDC_ADDRESS",
      "maxAmount": "1000000",
      "preApprovalExpiry": 281474976710655,
      "authorizationExpiry": 281474976710655,
      "refundExpiry": 281474976710655,
      "minFeeBps": 0,
      "maxFeeBps": 100,
      "feeReceiver": "0xOPERATOR_ADDRESS",
      "salt": "0x..."
    },
    "payer": "0xAGENT_ADDRESS",
    "amount": "1000000"
  },
  "paymentRequirements": {
    "scheme": "escrow",
    "network": "eip155:8453",
    "extra": {
      "escrowAddress": "0xESCROW_ADDRESS",
      "operatorAddress": "0xOPERATOR_ADDRESS",
      "tokenCollector": "0xTOKEN_COLLECTOR"
    }
  }
}
```

### POST /settle (refundInEscrow)

Return escrowed funds to the agent when task is cancelled. No ERC-3009 signature needed.

```json
{
  "x402Version": 2,
  "scheme": "escrow",
  "action": "refundInEscrow",
  "payload": {
    "paymentInfo": {
      "operator": "0xOPERATOR_ADDRESS",
      "receiver": "0xWORKER_ADDRESS",
      "token": "0xUSDC_ADDRESS",
      "maxAmount": "1000000",
      "preApprovalExpiry": 281474976710655,
      "authorizationExpiry": 281474976710655,
      "refundExpiry": 281474976710655,
      "minFeeBps": 0,
      "maxFeeBps": 100,
      "feeReceiver": "0xOPERATOR_ADDRESS",
      "salt": "0x..."
    },
    "payer": "0xAGENT_ADDRESS",
    "amount": "1000000"
  },
  "paymentRequirements": {
    "scheme": "escrow",
    "network": "eip155:8453",
    "extra": {
      "escrowAddress": "0xESCROW_ADDRESS",
      "operatorAddress": "0xOPERATOR_ADDRESS",
      "tokenCollector": "0xTOKEN_COLLECTOR"
    }
  }
}
```

### POST /escrow/state

Query the current state of an escrow payment (read-only, no gas).

**Request:**

```json
{
  "paymentInfo": {
    "operator": "0xOPERATOR_ADDRESS",
    "receiver": "0xWORKER_ADDRESS",
    "token": "0xUSDC_ADDRESS",
    "maxAmount": "1000000",
    "preApprovalExpiry": 281474976710655,
    "authorizationExpiry": 281474976710655,
    "refundExpiry": 281474976710655,
    "minFeeBps": 0,
    "maxFeeBps": 100,
    "feeReceiver": "0xOPERATOR_ADDRESS",
    "salt": "0x..."
  },
  "payer": "0xAGENT_ADDRESS",
  "network": "eip155:8453",
  "extra": {
    "escrowAddress": "0xESCROW_ADDRESS",
    "operatorAddress": "0xOPERATOR_ADDRESS",
    "tokenCollector": "0xTOKEN_COLLECTOR"
  }
}
```

**Response:**

```json
{
  "hasCollectedPayment": false,
  "capturableAmount": "1000000",
  "refundableAmount": "0",
  "paymentInfoHash": "0xabcdef...",
  "network": "eip155:8453"
}
```

## Supported Networks (CAIP-2)

| Network | CAIP-2 ID | Status |
|---------|-----------|--------|
| Base Mainnet | `eip155:8453` | Active |
| Base Sepolia | `eip155:84532` | Testnet |
| Ethereum | `eip155:1` | Active |
| Ethereum Sepolia | `eip155:11155111` | Testnet |
| Polygon | `eip155:137` | Active |
| Arbitrum | `eip155:42161` | Active |
| Celo | `eip155:42220` | Active |
| Monad | `eip155:143` | Active |
| Avalanche | `eip155:43114` | Active |

**Note:** PaymentOperators must be deployed on each network before escrow operations work. Check `/supported` for networks with active operators.

## Error Codes

| Error | Description |
|-------|-------------|
| `FeatureDisabled` | `ENABLE_PAYMENT_OPERATOR=true` not set |
| `UnsupportedNetwork` | Network has no escrow contracts or no deployed operator |
| `UnknownAction` | Invalid action (valid: authorize, release, refundInEscrow) |
| `PaymentInfoInvalid` | Address mismatch between request and known deployments |
| `ContractCall` | On-chain transaction reverted |
| `InvalidAmount` | Amount exceeds uint120 max (for refundInEscrow) |

## Security Model

1. **Address validation**: All client-provided addresses are validated against hardcoded contract addresses before any transaction is sent
2. **On-chain access control**: The PaymentOperator contract enforces `msg.sender == operator`, so only the facilitator can execute operations
3. **Replay protection**: Each escrow is uniquely identified by the `salt` in paymentInfo - replaying release/refund reverts on-chain
4. **Feature gating**: All escrow operations require `ENABLE_PAYMENT_OPERATOR=true`
