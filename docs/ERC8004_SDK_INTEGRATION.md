# ERC-8004 Trustless Agents SDK Integration Guide

This guide explains how to integrate ERC-8004 reputation feedback into your x402 SDK implementations.

## Overview

ERC-8004 "Trustless Agents" enables on-chain reputation tracking for AI agents. When integrated with x402:

1. **Settlement** returns a `ProofOfPayment` that cryptographically proves a payment occurred
2. **Feedback** allows submitting reputation scores on-chain using that proof

## Extension Identifier

The x402 extension ID for ERC-8004 is: `8004-reputation`

## Flow Diagram

```
┌─────────┐      ┌─────────────┐      ┌─────────────┐
│  Client │      │ Facilitator │      │  Blockchain │
└────┬────┘      └──────┬──────┘      └──────┬──────┘
     │                  │                    │
     │  1. Settle with  │                    │
     │  extension flag  │                    │
     ├─────────────────►│                    │
     │                  │  2. Execute        │
     │                  │  transfer          │
     │                  ├───────────────────►│
     │                  │                    │
     │  3. Return       │◄───────────────────┤
     │  ProofOfPayment  │                    │
     │◄─────────────────┤                    │
     │                  │                    │
     │  4. Submit       │                    │
     │  feedback        │                    │
     ├─────────────────►│                    │
     │                  │  5. Call           │
     │                  │  submitFeedback()  │
     │                  ├───────────────────►│
     │                  │                    │
     │  6. Confirmation │◄───────────────────┤
     │◄─────────────────┤                    │
     │                  │                    │
```

## Step 1: Request ProofOfPayment in Settlement

### TypeScript SDK

```typescript
import { x402 } from '@x402/sdk';

// Create payment requirements with ERC-8004 extension
const paymentRequirements = {
  scheme: 'exact',
  network: 'base-mainnet', // or 'eip155:8453' for v2
  maxAmountRequired: '1000000', // 1 USDC
  asset: '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913', // USDC on Base
  payTo: '0xYourAgentAddress',
  // Enable ERC-8004 extension
  extra: {
    '8004-reputation': {
      include_proof: true  // Request ProofOfPayment in response
    }
  }
};

// Execute settlement
const settleResponse = await x402.settle({
  facilitatorUrl: 'https://facilitator.ultravioletadao.xyz',
  paymentPayload: signedPayment,
  paymentRequirements
});

// Extract ProofOfPayment from response
if (settleResponse.success && settleResponse.proof_of_payment) {
  const proof = settleResponse.proof_of_payment;
  console.log('ProofOfPayment received:', proof);
  // Store proof for later feedback submission
}
```

### Go SDK

```go
package main

import (
    "github.com/x402/x402-go"
)

func main() {
    // Create payment requirements with ERC-8004 extension
    requirements := x402.PaymentRequirements{
        Scheme:            "exact",
        Network:           "base-mainnet",
        MaxAmountRequired: "1000000",
        Asset:             "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        PayTo:             "0xYourAgentAddress",
        Extra: map[string]interface{}{
            "8004-reputation": map[string]interface{}{
                "include_proof": true,
            },
        },
    }

    // Execute settlement
    response, err := client.Settle(ctx, x402.SettleRequest{
        PaymentPayload:      signedPayment,
        PaymentRequirements: requirements,
    })
    if err != nil {
        log.Fatal(err)
    }

    // Extract ProofOfPayment
    if response.Success && response.ProofOfPayment != nil {
        proof := response.ProofOfPayment
        fmt.Printf("ProofOfPayment: %+v\n", proof)
    }
}
```

### Python SDK

```python
from x402 import X402Client

client = X402Client(facilitator_url="https://facilitator.ultravioletadao.xyz")

# Create payment requirements with ERC-8004 extension
payment_requirements = {
    "scheme": "exact",
    "network": "base-mainnet",
    "maxAmountRequired": "1000000",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "payTo": "0xYourAgentAddress",
    "extra": {
        "8004-reputation": {
            "include_proof": True
        }
    }
}

# Execute settlement
response = client.settle(
    payment_payload=signed_payment,
    payment_requirements=payment_requirements
)

# Extract ProofOfPayment
if response["success"] and "proof_of_payment" in response:
    proof = response["proof_of_payment"]
    print(f"ProofOfPayment received: {proof}")
```

## Step 2: Submit Feedback

After receiving a `ProofOfPayment`, you can submit reputation feedback to the ERC-8004 Reputation Registry.

### ProofOfPayment Structure

```typescript
interface ProofOfPayment {
  transaction_hash: string;    // Settlement tx hash
  block_number: number;        // Block where settlement occurred
  network: string;             // Network identifier
  payer: string;               // Address that paid
  payee: string;               // Address that received payment
  amount: string;              // Amount in base units
  token: string;               // Token contract address
  timestamp: number;           // Unix timestamp
  payment_hash: string;        // Keccak256 hash of payment data
}
```

### Feedback Request Structure

```typescript
interface FeedbackRequest {
  x402_version: 1 | 2;
  network: string;             // Must match proof.network
  feedback: {
    agent: string;             // Agent address receiving feedback
    score: number;             // 1-5 rating
    comment?: string;          // Optional review text
    proof: ProofOfPayment;     // From settlement response
    task_id?: string;          // Optional task identifier
  }
}
```

### TypeScript SDK

```typescript
// Submit feedback using the ProofOfPayment from settlement
const feedbackResponse = await fetch(
  'https://facilitator.ultravioletadao.xyz/feedback',
  {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      x402_version: 1,
      network: 'base-mainnet',
      feedback: {
        agent: '0xAgentAddress',
        score: 5,
        comment: 'Excellent service, fast response!',
        proof: settleResponse.proof_of_payment,
        task_id: 'task-12345' // optional
      }
    })
  }
);

const result = await feedbackResponse.json();
if (result.success) {
  console.log('Feedback submitted! TX:', result.transaction);
} else {
  console.error('Feedback failed:', result.error);
}
```

### Go SDK

```go
// Prepare feedback request
feedbackReq := FeedbackRequest{
    X402Version: 1,
    Network:     "base-mainnet",
    Feedback: FeedbackData{
        Agent:   "0xAgentAddress",
        Score:   5,
        Comment: "Excellent service!",
        Proof:   response.ProofOfPayment,
        TaskID:  "task-12345",
    },
}

// Submit feedback
feedbackResp, err := http.Post(
    "https://facilitator.ultravioletadao.xyz/feedback",
    "application/json",
    bytes.NewBuffer(jsonData),
)
if err != nil {
    log.Fatal(err)
}
```

### Python SDK

```python
import requests

# Submit feedback
feedback_response = requests.post(
    "https://facilitator.ultravioletadao.xyz/feedback",
    json={
        "x402_version": 1,
        "network": "base-mainnet",
        "feedback": {
            "agent": "0xAgentAddress",
            "score": 5,
            "comment": "Excellent service!",
            "proof": proof,  # From settlement response
            "task_id": "task-12345"
        }
    }
)

result = feedback_response.json()
if result["success"]:
    print(f"Feedback submitted! TX: {result['transaction']}")
else:
    print(f"Feedback failed: {result['error']}")
```

### cURL Example

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/feedback \
  -H "Content-Type: application/json" \
  -d '{
    "x402_version": 1,
    "network": "base-mainnet",
    "feedback": {
      "agent": "0xAgentAddress",
      "score": 5,
      "comment": "Great AI agent!",
      "proof": {
        "transaction_hash": "0x123...",
        "block_number": 12345678,
        "network": "base-mainnet",
        "payer": "0xPayer...",
        "payee": "0xPayee...",
        "amount": "1000000",
        "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        "timestamp": 1706500000,
        "payment_hash": "0xabc..."
      }
    }
  }'
```

## Feedback Response

```typescript
interface FeedbackResponse {
  success: boolean;
  transaction?: string;    // On-chain tx hash if successful
  error?: string;          // Error message if failed
  network: string;         // Network where feedback was submitted
}
```

## Supported Networks

ERC-8004 reputation feedback is supported on:

### Testnets (Available Now)
- `base-sepolia` / `eip155:84532`
- `ethereum-sepolia` / `eip155:11155111`
- `optimism-sepolia` / `eip155:11155420`

### Mainnets (Thursday January 29, 2025)
- `base-mainnet` / `eip155:8453`
- `ethereum-mainnet` / `eip155:1`
- `optimism-mainnet` / `eip155:10`

## Score Guidelines

| Score | Meaning |
|-------|---------|
| 1 | Very Poor - Agent failed to complete task, unresponsive |
| 2 | Poor - Significant issues, partial completion |
| 3 | Average - Task completed with some issues |
| 4 | Good - Task completed well, minor issues |
| 5 | Excellent - Perfect execution, exceeded expectations |

## Error Handling

### Common Errors

| Error | Cause | Solution |
|-------|-------|----------|
| `ERC-8004 not supported on network` | Network doesn't have ERC-8004 contracts | Use supported network |
| `Agent address must be an EVM address` | Non-EVM address provided | Use Ethereum-style address |
| `Invalid proof of payment` | Proof doesn't match on-chain data | Verify proof from settlement |
| `ERC-8004 contracts not configured` | Contracts not yet deployed | Wait for mainnet launch |

### Retry Logic

```typescript
async function submitFeedbackWithRetry(
  feedback: FeedbackRequest,
  maxRetries = 3
): Promise<FeedbackResponse> {
  for (let i = 0; i < maxRetries; i++) {
    try {
      const response = await fetch(
        'https://facilitator.ultravioletadao.xyz/feedback',
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(feedback)
        }
      );

      const result = await response.json();
      if (result.success) return result;

      // Don't retry validation errors
      if (response.status === 400) throw new Error(result.error);

    } catch (error) {
      if (i === maxRetries - 1) throw error;
      await new Promise(r => setTimeout(r, 1000 * (i + 1)));
    }
  }
}
```

## Contract Addresses

After mainnet launch (Thursday January 29, 2025), contract addresses will be available at:

```
GET https://facilitator.ultravioletadao.xyz/feedback
```

Response includes `contracts.reputationRegistry` with the deployed address.

## Reading Reputation Data

To read an agent's reputation score directly from the blockchain:

```typescript
import { ethers } from 'ethers';

const REPUTATION_REGISTRY_ABI = [
  'function getReputation(address agent) view returns (uint256 score, uint256 feedbackCount)',
  'function getFeedback(address agent, uint256 index) view returns (address rater, uint8 score, string comment, bytes32 proofHash, uint256 timestamp)'
];

async function getAgentReputation(agentAddress: string, network: string) {
  const provider = new ethers.JsonRpcProvider(getRpcUrl(network));
  const registryAddress = await getReputationRegistryAddress();

  const registry = new ethers.Contract(
    registryAddress,
    REPUTATION_REGISTRY_ABI,
    provider
  );

  const [score, feedbackCount] = await registry.getReputation(agentAddress);

  return {
    averageScore: Number(score) / 100, // Stored as score * 100
    totalFeedback: Number(feedbackCount)
  };
}
```

## Best Practices

1. **Always store ProofOfPayment** - Save the proof immediately after settlement for later feedback submission

2. **Validate before submitting** - Ensure the proof network matches the feedback network

3. **Handle gas costs** - The facilitator pays gas for feedback submission; no additional cost to clients

4. **Rate limiting** - One feedback per settlement transaction; duplicate submissions will fail

5. **Privacy considerations** - Feedback is public on-chain; avoid including sensitive information in comments

## Resources

- [ERC-8004 Specification](https://eips.ethereum.org/EIPS/eip-8004)
- [x402 Protocol Documentation](https://x402.org)
- [Facilitator API Reference](https://facilitator.ultravioletadao.xyz)

## Support

For integration support:
- GitHub Issues: https://github.com/UltravioletaDAO/x402-rs/issues
- Discord: [Ultravioleta DAO](https://discord.gg/ultravioleta)
