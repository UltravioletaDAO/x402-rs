# ERC-8004 Solana Agent Registry SDK Guide

This guide explains how to interact with the Solana Agent Registry (ERC-8004 on Solana) using available SDKs and the x402-rs facilitator API.

## Overview

The Solana Agent Registry provides on-chain identity, reputation, and trust infrastructure for AI agents. Available integration paths:

1. **x402-rs Facilitator API** -- RESTful endpoints for identity and reputation queries
2. **TypeScript SDK** (`8004-solana`) -- Direct program interaction from Node.js/browser
3. **MCP Server** (`@quantulabs/8004-mcp`) -- Multi-chain agent discovery via Model Context Protocol

---

## TypeScript SDK (8004-solana)

### Installation

```bash
npm install 8004-solana
```

### Initialization

```typescript
import { SolanaSDK } from '8004-solana';
import { Keypair } from '@solana/web3.js';

const sdk = new SolanaSDK({
  cluster: 'mainnet-beta', // or 'devnet' | 'localnet'
  signer: keypair,
  ipfsClient: new IPFSClient({
    pinataEnabled: true,
    pinataJwt: process.env.PINATA_JWT,
  }),
});
```

### Register an Agent

```typescript
const result = await sdk.registerAgent('https://example.com/agent-registration.json', {
  name: 'My AI Agent',
  metadata: [
    { key: 'x402Support', value: 'true' },
    { key: 'protocol', value: 'A2A' },
  ],
});

console.log('Agent NFT:', result.asset.toBase58());
// Agent NFT: 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv
```

### Submit Feedback

```typescript
await sdk.giveFeedback(assetPubkey, {
  value: '85',
  tag1: 'x402-resource-delivered',
  tag2: 'exact-svm',
  feedbackUri: 'ipfs://QmFeedbackHash',
  score: 85,
});
```

### Query Reputation

```typescript
const summary = await sdk.getSummary(assetPubkey);
console.log('Trust Tier:', summary.trustTier);
console.log('Quality Score:', summary.qualityScore);
console.log('Feedback Count:', summary.feedbackCount);
```

### Check Agent Liveness

```typescript
const status = await sdk.isItAlive(assetPubkey);
// Returns: {
//   status: 'live' | 'partially' | 'not_live',
//   liveServices: ['A2A', 'MCP'],
//   deadServices: []
// }
```

### Sign with Agent Identity

```typescript
const signed = await sdk.sign(assetPubkey, data);
const isValid = await sdk.verify(signedObject, assetPubkey);
```

---

## MCP Server (@quantulabs/8004-mcp)

The MCP server provides multi-chain agent discovery tools for AI assistants.

### Quick Start

```bash
npx @quantulabs/8004-mcp
```

### Configuration

Add to your MCP server configuration:

```json
{
  "mcpServers": {
    "8004-registry": {
      "command": "npx",
      "args": ["@quantulabs/8004-mcp"],
      "env": {
        "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com"
      }
    }
  }
}
```

### Available Tools

- **search_agents** -- Search agents by name, capabilities, or metadata across Solana and EVM chains
- **get_agent** -- Get full agent identity, services, and reputation
- **list_agents** -- List registered agents with pagination
- **get_reputation** -- Get ATOM Engine reputation stats for a Solana agent

### x402 Integration

The MCP server supports the `8004-reputation` x402 extension, enabling feedback linked to actual payment transactions for verifiable proof-of-payment reputation.

---

## Facilitator API Usage

The x402-rs facilitator exposes the same endpoints for Solana and EVM networks. Use network `"solana"` or `"solana-devnet"` in requests.

### Query Identity

```bash
curl -s https://facilitator.ultravioletadao.xyz/identity/solana/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv | jq
```

### Query Reputation

```bash
curl -s https://facilitator.ultravioletadao.xyz/reputation/solana/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv | jq
```

Response includes ATOM Engine stats not available on EVM:
```json
{
  "atomStats": {
    "trustTier": 3,
    "trustTierName": "Trusted",
    "qualityScore": 78,
    "confidence": 85,
    "riskScore": 12,
    "diversityRatio": 67
  }
}
```

### Submit Feedback After x402 Payment

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/feedback \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "solana",
    "feedback": {
      "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
      "value": 92,
      "valueDecimals": 0,
      "tag1": "x402-resource-delivered",
      "tag2": "exact-svm"
    }
  }'
```

### Query Total Registered Agents

```bash
curl -s https://facilitator.ultravioletadao.xyz/identity/solana/total-supply | jq
```

---

## ProofOfPayment Flow

When an x402 payment is settled on Solana, the facilitator generates a `ProofOfPayment` that can be used for reputation feedback:

```
1. Client sends x402 payment on Solana
   -> Facilitator settles transferWithAuthorization
   -> Returns Solana transaction signature (64-byte Ed25519)

2. Client includes tx signature in ProofOfPayment
   {
     "transactionHash": "5UfDuX...",
     "network": "solana",
     "payer": "BuyerPubkey...",
     "payee": "AgentPubkey...",
     "amount": "1000000",
     "token": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
   }

3. Client calls POST /feedback with proof
   -> Facilitator verifies proof and submits feedback on-chain

4. ATOM Engine updates via CPI:
   -> trust_tier may upgrade
   -> quality_score recalculated (EMA)
   -> HyperLogLog updated (unique client tracking)
   -> Ring buffer updated (burst detection)
```

---

## Standardized Tags for x402

### Client-to-Server Feedback (tag1)

| Tag | Meaning |
|-----|---------|
| `x402-resource-delivered` | Resource was delivered successfully |
| `x402-delivery-failed` | Resource was not delivered |
| `x402-delivery-timeout` | Delivery timed out |
| `x402-quality-issue` | Delivered but with quality problems |

### Server-to-Client Feedback (tag1)

| Tag | Meaning |
|-----|---------|
| `x402-good-payer` | Payment was successful |
| `x402-payment-failed` | Payment failed |
| `x402-insufficient-funds` | Insufficient balance |
| `x402-invalid-signature` | Bad signature |

### Network Identifier (tag2)

| Tag | Network |
|-----|---------|
| `exact-svm` | Solana (SVM) |
| `exact-evm` | EVM chains |

---

## References

- [Solana Agent Registry](https://solana.com/agent-registry)
- [QuantuLabs 8004-solana](https://github.com/QuantuLabs/8004-solana)
- [QuantuLabs 8004-solana-ts](https://github.com/QuantuLabs/8004-solana-ts)
- [QuantuLabs 8004-atom](https://github.com/QuantuLabs/8004-atom)
- [8004.qnt.sh](https://8004.qnt.sh)
- [EIP-8004 Specification](https://eips.ethereum.org/EIPS/eip-8004)
- [@quantulabs/8004-mcp](https://www.npmjs.com/package/@quantulabs/8004-mcp)
- [8004-solana npm](https://www.npmjs.com/package/8004-solana)
- [ERC8004_SOLANA_INTEGRATION.md](./ERC8004_SOLANA_INTEGRATION.md) -- Detailed technical integration guide
