# Handoff: Add `getIdentityByOwner()` to Python and TypeScript SDKs

## New Facilitator Endpoint (v1.41.1)

```
GET /identity/{network}/owner/{address}
```

Resolves the first ERC-8004 agent ID owned by a wallet address on a given network.

### Example Request

```
GET https://facilitator.ultravioletadao.xyz/identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15
```

### Example Response

```json
{
  "agentId": 246,
  "owner": "0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15",
  "agentUri": "https://execution.market/workers/0x52e05c8e45a32eee169639f6d2ca40f8887b5a15",
  "network": "skale-base",
  "balance": "5"
}
```

### Error Responses

- `400` — Invalid network or address
- `404` — Address does not own any agent on that network (`balance: 0`)
- `500` — Provider error

### Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `agentId` | number | First (lowest) token ID owned by this address |
| `owner` | string | The queried address (checksummed) |
| `agentUri` | string | Agent's registration URI (may be empty) |
| `network` | string | Network name |
| `balance` | string | Total number of agent NFTs owned (as string) |

## What Each SDK Needs

### Method Signature

```
getIdentityByOwner(network: string, address: string) -> IdentityByOwnerResponse
```

### Implementation

Simple GET request to `{baseUrl}/identity/{network}/owner/{address}`. No auth, no body, no headers beyond the standard ones.

### Response Type

Add a new type/class (or reuse existing identity types if compatible):

```
IdentityByOwnerResponse {
  agentId: number
  owner: string
  agentUri: string
  network: string
  balance: string
}
```

### Python SDK specifics

- Location: `/mnt/z/ultravioleta/dao/uvd-x402-sdk-python`
- Current version: v0.16.0
- Follow existing patterns (look at `get_identity()` or similar methods)

### TypeScript SDK specifics

- Location: `/mnt/z/ultravioleta/dao/uvd-x402-sdk-typescript`
- Current version: v2.28.0
- Follow existing patterns (look at `getIdentity()` or similar methods)

### Also Update `/register` Idempotency Awareness

The facilitator's `POST /register` is now idempotent — if the recipient already owns an agent on the target network, it returns the existing one instead of minting a duplicate. The SDKs don't need code changes for this (same request/response shape), but update any docs or comments that say "register always creates a new agent".

## Testing

```bash
# Verify endpoint works
curl -s https://facilitator.ultravioletadao.xyz/identity/skale-base/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15

# 404 case
curl -s https://facilitator.ultravioletadao.xyz/identity/base-mainnet/owner/0x0000000000000000000000000000000000000001

# Bad network
curl -s https://facilitator.ultravioletadao.xyz/identity/fake-network/owner/0x52E05C8e45a32eeE169639F6d2cA40f8887b5A15
```
