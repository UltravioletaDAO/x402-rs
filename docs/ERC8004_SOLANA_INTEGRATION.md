# ERC-8004 Solana Agent Registry Integration

## Overview

The x402-rs facilitator supports the Solana implementation of ERC-8004 (Trustless Agents) via the [QuantuLabs 8004-solana](https://github.com/QuantuLabs/8004-solana) Anchor program. This enables AI agents on Solana to register identities, accumulate reputation, and build trust -- all on-chain.

The [Solana Agent Registry](https://solana.com/agent-registry) was officially launched by the Solana Foundation on March 2, 2026. It is the Solana-native implementation of the ERC-8004 open protocol for agent discovery and trust.

This document covers the Solana-specific architecture. For EVM integration, see [ERC8004_INTEGRATION.md](./ERC8004_INTEGRATION.md).

---

## Program IDs

| Program | Mainnet | Devnet |
|---------|---------|--------|
| **Agent Registry** | `8oo4dC4JvBLwy5tGgiH3WwK4B9PWxL9Z4XjA2jzkQMbQ` | `8oo4J9tBB3Hna1jRQ3rWvJjojqM5DYTDJo5cejUuJy3C` |
| **ATOM Engine** | `AToMw53aiPQ8j7iHVb4fGt6nzUNxUhcPc3tbPBZuzVVb` | `AToMufS4QD6hEXvcvBDg9m1AHeCLpmZQsyfYa5h9MwAF` |

Built with **Anchor 0.31.1**. The Agent Registry handles identity and feedback operations. The ATOM Engine is a separate program called via CPI that computes on-chain reputation analytics.

---

## Architecture: EVM vs Solana

| Aspect | EVM (ERC-8004) | Solana (8004-solana) |
|--------|----------------|---------------------|
| **Identity** | ERC-721 NFT | Metaplex Core NFT in single collection |
| **Agent ID** | `uint256` (sequential) | `Pubkey` (base58, NFT mint address) |
| **Reputation storage** | On-chain state (contract mappings) | Event-only + SEAL v1 hash-chain |
| **Reputation scoring** | Off-chain aggregation | ATOM Engine (on-chain CPI program) |
| **Feedback queries** | `readAllFeedback()` view call | Indexer-based (events + SEAL verification) |
| **Sybil resistance** | None built-in | HyperLogLog + ring buffer + trust tiers |
| **Registration cost** | Gas-dependent ($0.50-$5 on L1) | ~0.006 SOL (~$0.80) |
| **Feedback cost** | Gas-dependent | ~0.005 SOL (~$0.65) |
| **Validation Registry** | Deployed on testnets | Not yet implemented |
| **Contract ABIs** | Alloy `sol!` macro | Anchor instructions (Borsh serialization) |

---

## Account Structures

### AgentAccount (313 bytes)

**PDA Seeds:** `["agent", asset.key()]`

| Field | Type | Size | Description |
|-------|------|------|-------------|
| owner | Pubkey | 32 | NFT owner address |
| asset | Pubkey | 32 | Metaplex Core NFT mint address (unique identifier) |
| bump | u8 | 1 | PDA bump seed |
| agent_uri | String | 204 | URI to agent registration file (IPFS/HTTPS) |
| nft_name | String | 36 | Human-readable agent name |
| feedback_digest | [u8; 32] | 32 | Rolling hash chain for feedback integrity |
| feedback_count | u64 | 8 | Total feedback received |
| response_digest | [u8; 32] | 32 | Rolling hash chain for responses |
| response_count | u64 | 8 | Total responses appended |
| revoke_digest | [u8; 32] | 32 | Rolling hash chain for revocations |
| revoke_count | u64 | 8 | Total feedback revocations |

### AtomStats (460 bytes)

**PDA Seeds:** `["atom_stats", asset.key()]`

| Field | Type | Size | Description |
|-------|------|------|-------------|
| collection | Pubkey | 32 | Registry collection address |
| asset | Pubkey | 32 | Agent NFT mint address |
| feedback_count | u32 | 4 | Total feedback count |
| positive_count | u32 | 4 | Positive feedback count |
| negative_count | u32 | 4 | Negative feedback count |
| quality_score | i32 | 4 | EMA quality score (centered at 0) |
| last_feedback_slot | u64 | 8 | Slot of most recent feedback |
| hll_packed | [u8; 128] | 128 | HyperLogLog registers (256 x 4-bit) |
| hll_salt | u64 | 8 | Per-agent salt for HLL grinding prevention |
| recent_callers | [u64; 24] | 192 | Ring buffer for burst detection |
| eviction_cursor | u8 | 1 | Ring buffer cursor |
| trust_tier | u8 | 1 | Trust level (0-4) |
| confidence | u8 | 1 | Statistical confidence |
| risk_score | u8 | 1 | Risk assessment |
| diversity_ratio | u8 | 1 | Client diversity measure |
| bump | u8 | 1 | PDA bump seed |

### MetadataEntryPda (~332 bytes)

**PDA Seeds:** `["agent_meta", asset.key(), key_hash[0..8]]`

Where `key_hash` = first 8 bytes of `SHA256(metadata_key)`.

| Field | Type | Size | Description |
|-------|------|------|-------------|
| asset | Pubkey | 32 | Agent NFT mint address |
| metadata_key | String | 36 | Metadata key name |
| metadata_value | Vec\<u8\> | 254 | Metadata value (arbitrary bytes) |
| immutable | bool | 1 | If true, cannot be modified or deleted |
| bump | u8 | 1 | PDA bump seed |

### RegistryConfig (78 bytes)

**PDA Seeds:** `["config"]`

| Field | Type | Size | Description |
|-------|------|------|-------------|
| collection | Pubkey | 32 | Metaplex Core collection address |
| registry_type | u8 | 1 | Registry type identifier |
| authority | Pubkey | 32 | Upgrade authority |
| base_index | u32 | 4 | Total registered agents (sequential counter) |
| bump | u8 | 1 | PDA bump seed |

---

## PDA Derivation Map

| Account | Seeds | Program |
|---------|-------|---------|
| RegistryConfig | `["config"]` | Agent Registry |
| AgentAccount | `["agent", asset_pubkey]` | Agent Registry |
| MetadataEntryPda | `["agent_meta", asset_pubkey, sha256(key)[0..8]]` | Agent Registry |
| AtomStats | `["atom_stats", asset_pubkey]` | ATOM Engine |
| ATOM Config | `["atom_config"]` | ATOM Engine |

All PDAs use `asset.key()` (the Metaplex Core NFT mint address) as the primary seed, replacing the sequential `agent_id` used in EVM.

---

## SEAL v1 (Solana Event Authenticity Layer)

SEAL v1 provides trustless integrity verification for event-based data on Solana. Since feedback is stored as events (not on-chain state), SEAL ensures data integrity through deterministic hash chains.

### How It Works

Each feedback, response, or revocation operation updates a rolling hash digest on-chain:

```
new_digest = keccak256(prev_digest || DOMAIN_CONSTANT || leaf_hash)
leaf_hash  = keccak256(DOMAIN_LEAF || asset || client || index || seal_hash || slot)
```

**Domain Constants:**
- `8004_SEAL_V1____` (16 bytes) -- domain separator for chain updates
- `8004_LEAF_V1____` (16 bytes) -- domain separator for leaf construction

### Three Independent Chains

| Chain | Updated By | Purpose |
|-------|-----------|---------|
| `feedback_digest` | `give_feedback()` | Tracks all feedback submissions |
| `response_digest` | `append_response()` | Tracks all response attachments |
| `revoke_digest` | `revoke_feedback()` | Tracks all revocations |

### Verification

The [8004-solana-indexer](https://github.com/QuantuLabs/8004-solana-indexer) replays all historical events and recomputes the expected digest. If the computed digest matches the on-chain value, the data is verified intact. Any missing or tampered events would produce a different hash.

The `seal_hash` for each operation is computed **on-chain** from all instruction parameters, making it impossible for clients to misrepresent submitted data.

---

## ATOM Engine Trust Tiers

The ATOM Engine computes reputation analytics on-chain via CPI from the Agent Registry.

### Trust Tier System

| Tier | Value | Upgrade Requirements | Downgrade Threshold |
|------|-------|---------------------|---------------------|
| Unknown | 0 | -- | -- |
| New | 1 | 1 feedback | 0 |
| Established | 2 | 10 feedbacks + quality >= 60 | quality < 50 |
| Trusted | 3 | 50 feedbacks + quality >= 75 | quality < 65 |
| Legendary | 4 | 200 feedbacks + quality >= 90 | quality < 80 |

Hysteresis prevents oscillation near tier boundaries.

### Anti-Sybil Mechanisms

**HyperLogLog (Unique Client Estimation):**
- 256 registers, 4-bit packed (128 bytes total)
- ~6.5% standard error
- Per-agent salt prevents HLL grinding attacks
- Estimates number of unique feedback providers

**Ring Buffer (Burst Detection):**
- 24 slots with 56-bit fingerprints
- Round-robin eviction via cursor
- Detects rapid-fire feedback from the same client

### EMA Quality Score

Exponential Moving Average with alpha=0.1 (10% weight to new scores):

```
centered = (score as i32) - 50
quality_score = (quality_score * 900 + centered * 100) / 1000
```

The quality score centers at 0 (representing a neutral 50/100 score). Positive values indicate above-average performance.

---

## API Endpoints

The facilitator exposes the same endpoints for Solana as for EVM. The network parameter distinguishes the chain.

### Query Agent Identity

```bash
# Mainnet
curl -s https://facilitator.ultravioletadao.xyz/identity/solana/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv

# Devnet
curl -s https://facilitator.ultravioletadao.xyz/identity/solana-devnet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv
```

Example response:
```json
{
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "owner": "5FHwkrdxPMsgAJBDkWmcoLiN9m1K95VCGw7qr4eXfjsP",
  "agentUri": "https://example.com/.well-known/agent-registration.json",
  "agentWallet": null,
  "network": "solana"
}
```

### Query Agent Reputation

```bash
curl -s https://facilitator.ultravioletadao.xyz/reputation/solana/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv
```

Example response (includes ATOM Engine bonus data):
```json
{
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "summary": {
    "count": 47,
    "summaryValue": 4230,
    "summaryValueDecimals": 2
  },
  "atomStats": {
    "trustTier": 3,
    "trustTierName": "Trusted",
    "qualityScore": 78,
    "confidence": 85,
    "riskScore": 12,
    "diversityRatio": 67,
    "positiveCount": 42,
    "negativeCount": 5,
    "feedbackCount": 47,
    "lastFeedbackSlot": 312456789
  },
  "network": "solana"
}
```

Note: The `atomStats` field is only present for Solana networks. EVM networks return `null` for this field since they use off-chain aggregation.

### Query Agent Metadata

```bash
curl -s https://facilitator.ultravioletadao.xyz/identity/solana/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv/metadata/x402Support
```

### Query Total Registered Agents

```bash
curl -s https://facilitator.ultravioletadao.xyz/identity/solana/total-supply
```

### Submit Feedback (Phase 2)

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/feedback \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "network": "solana",
    "feedback": {
      "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
      "value": 87,
      "valueDecimals": 0,
      "tag1": "x402-resource-delivered",
      "tag2": "exact-svm",
      "endpoint": "https://api.example.com/agent",
      "feedbackUri": "",
      "proof": {
        "transactionHash": "5UfDuX...",
        "network": "solana",
        "payer": "BuyerPubkey...",
        "payee": "AgentPubkey...",
        "amount": "1000000",
        "token": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
      }
    }
  }'
```

---

## Cost Analysis

| Operation | Cost (SOL) | Approx. USD | Notes |
|-----------|-----------|-------------|-------|
| Register Agent | ~0.0058 | ~$0.75 | Metaplex Core asset + AgentAccount PDA |
| Initialize ATOM Stats | ~0.005 | ~$0.65 | Per-agent analytics account |
| Give Feedback | ~0.0046 | ~$0.60 | Feedback event + ATOM Engine CPI update |
| Set Metadata | ~0.0032 | ~$0.42 | MetadataEntryPda creation |
| Append Response | ~0.0012 | ~$0.16 | Response event (minimal) |
| Delete Metadata | ~0.0029 refund | -- | Rent recovery |

USD estimates based on SOL ~$130.

---

## Differences from EVM Implementation

### Agent ID Format

- **EVM**: Sequential `uint256` (e.g., `42`, `1337`)
- **Solana**: Base58 `Pubkey` (e.g., `7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv`)

The Solana agent ID is the Metaplex Core NFT mint address, which serves as the unique identifier for all PDA derivations.

### Reputation Data Richness

On EVM, `getSummary()` returns only `count`, `summaryValue`, and `summaryValueDecimals`. On Solana, the ATOM Engine provides additional on-chain analytics:

- `trustTier` (0-4): Computed trust level with vesting
- `qualityScore`: EMA quality with hysteresis
- `confidence`: Statistical confidence
- `riskScore`: Risk assessment
- `diversityRatio`: Client diversity via HyperLogLog

### Historical Feedback

On EVM, `readAllFeedback()` returns all historical feedback directly from contract state. On Solana, feedback is stored as events and requires an indexer (the [8004-solana-indexer](https://github.com/QuantuLabs/8004-solana-indexer)) for historical queries. The facilitator currently returns aggregated data from AtomStats for Solana.

### Validation Registry

The Validation Registry is available on EVM testnets but has not yet been implemented on Solana.

---

## References

- [Solana Agent Registry](https://solana.com/agent-registry) -- Official Solana Foundation page
- [QuantuLabs 8004-solana](https://github.com/QuantuLabs/8004-solana) -- Anchor program source
- [QuantuLabs 8004-solana-ts](https://github.com/QuantuLabs/8004-solana-ts) -- TypeScript SDK
- [QuantuLabs 8004-atom](https://github.com/QuantuLabs/8004-atom) -- ATOM Engine program
- [QuantuLabs 8004-solana-indexer](https://github.com/QuantuLabs/8004-solana-indexer) -- SEAL v1 indexer
- [8004.qnt.sh](https://8004.qnt.sh) -- QuantuLabs documentation
- [EIP-8004 Specification](https://eips.ethereum.org/EIPS/eip-8004) -- Original Ethereum standard
- [@quantulabs/8004-mcp](https://www.npmjs.com/package/@quantulabs/8004-mcp) -- Multi-chain MCP server
- [8004-solana npm](https://www.npmjs.com/package/8004-solana) -- TypeScript SDK on npm
