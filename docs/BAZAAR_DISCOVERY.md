# Bazaar Discovery API

The x402 v2 Bazaar Discovery system allows clients to discover paid API endpoints and services registered with the facilitator.

## Overview

Bazaar is an in-memory discovery registry that enables:

- **Resource Discovery**: Clients can query for available paid endpoints
- **Self-Registration**: The facilitator registers itself as a discoverable resource
- **Provider Registration**: Third-party services can register their x402-enabled endpoints

## API Endpoints

### GET /discovery/resources

List all discoverable resources with optional filtering and pagination.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `limit` | u32 | Max items to return (default: 10, max: 100) |
| `offset` | u32 | Number of items to skip (default: 0) |
| `category` | string | Filter by category (e.g., "finance", "ai") |
| `network` | string | Filter by network (e.g., "eip155:8453") |

**Example Request:**
```bash
curl https://facilitator.ultravioletadao.xyz/discovery/resources
```

**Example Response:**
```json
{
  "x402Version": 2,
  "items": [
    {
      "url": "https://facilitator.ultravioletadao.xyz/",
      "type": "facilitator",
      "x402Version": 2,
      "description": "Ultravioleta DAO x402 Payment Facilitator - supports 28 networks for gasless micropayments",
      "accepts": [],
      "lastUpdated": 1766458335,
      "metadata": {
        "category": "payment-facilitator",
        "provider": "Ultravioleta DAO",
        "tags": ["x402", "facilitator", "gasless", "micropayments", "evm", "solana"]
      }
    }
  ],
  "pagination": {
    "limit": 10,
    "offset": 0,
    "total": 1
  }
}
```

### POST /discovery/register

Register a new resource in the discovery registry.

**Request Body:**
```json
{
  "url": "https://api.example.com/premium-data",
  "type": "http",
  "description": "Premium market data API with real-time updates",
  "accepts": [
    {
      "scheme": "exact",
      "network": "eip155:8453",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "amount": "10000",
      "payTo": "0x1234567890123456789012345678901234567890",
      "maxTimeoutSeconds": 60
    }
  ],
  "metadata": {
    "category": "finance",
    "provider": "Example Corp",
    "tags": ["market-data", "real-time"]
  }
}
```

**Response:** `201 Created` or `400 Bad Request`

### GET /supported

Returns supported payment kinds with Bazaar extension declaration.

**Example Response:**
```json
{
  "x402Version": 2,
  "kinds": [...],
  "extensions": ["bazaar"],
  "signers": {}
}
```

## Resource Types

| Type | Description |
|------|-------------|
| `http` | Standard HTTP API endpoints |
| `mcp` | Model Context Protocol servers |
| `a2a` | Agent-to-Agent protocol endpoints |
| `facilitator` | x402 payment facilitator services |

## Self-Registration

The facilitator automatically registers itself on startup when `FACILITATOR_URL` is configured:

```bash
# In .env or ECS task definition
FACILITATOR_URL=https://facilitator.ultravioletadao.xyz
```

When set, the facilitator:
1. Reads its own `/supported` endpoint to count available networks
2. Creates a `DiscoveryResource` with type `facilitator`
3. Registers itself in the discovery registry
4. Logs: `Self-registered facilitator at https://...`

## Architecture

### Storage

The registry uses in-memory storage with thread-safe access:

```rust
pub struct DiscoveryRegistry {
    resources: Arc<RwLock<HashMap<String, DiscoveryResource>>>,
}
```

**Note:** Resources are lost on service restart. Future versions may add persistent storage.

### Data Types

```rust
pub struct DiscoveryResource {
    pub url: Url,
    pub resource_type: String,
    pub x402_version: u8,
    pub description: String,
    pub accepts: Vec<PaymentRequirementsV2>,
    pub last_updated: u64,
    pub metadata: Option<DiscoveryMetadata>,
}

pub struct DiscoveryMetadata {
    pub category: Option<String>,
    pub provider: Option<String>,
    pub tags: Vec<String>,
}

pub struct DiscoveryResponse {
    pub x402_version: u8,
    pub items: Vec<DiscoveryResource>,
    pub pagination: Pagination,
}
```

## Configuration

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `FACILITATOR_URL` | No | Public URL for self-registration |

### ECS Task Definition

Add to the `environment` section:
```json
{
  "name": "FACILITATOR_URL",
  "value": "https://facilitator.ultravioletadao.xyz"
}
```

## Validation Rules

Resources must pass validation before registration:

1. **URL**: Must be valid HTTPS (or HTTP for localhost)
2. **Type**: Must be one of: `http`, `mcp`, `a2a`, `facilitator`
3. **Accepts**: Must have at least one payment method (except `facilitator` type)
4. **Duplicates**: Cannot register the same URL twice (use update instead)

## Example Use Cases

### 1. Discovering AI Services

```bash
curl "https://facilitator.ultravioletadao.xyz/discovery/resources?category=ai"
```

### 2. Finding Services on Base Network

```bash
curl "https://facilitator.ultravioletadao.xyz/discovery/resources?network=eip155:8453"
```

### 3. Registering a Paid API

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/discovery/register \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://myapi.com/v1/data",
    "type": "http",
    "description": "My premium data API",
    "accepts": [{
      "scheme": "exact",
      "network": "eip155:8453",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "amount": "100000",
      "payTo": "0xYourWalletAddress",
      "maxTimeoutSeconds": 60
    }],
    "metadata": {
      "category": "data",
      "provider": "My Company",
      "tags": ["api", "data"]
    }
  }'
```

## Security Considerations

1. **Open Registration**: Currently no authentication required. Add rate limiting if abuse occurs.
2. **URL Validation**: Only HTTPS URLs accepted (HTTP allowed for localhost development).
3. **No Persistence**: In-memory storage means attackers cannot permanently pollute the registry.

## Future Enhancements

- Persistent storage (PostgreSQL/Redis)
- API key authentication for registration
- Resource expiry/TTL
- Webhook notifications for new registrations
- Search by payment amount range

## Related Files

| File | Purpose |
|------|---------|
| `src/discovery.rs` | DiscoveryRegistry implementation |
| `src/types_v2.rs` | Discovery types and serialization |
| `src/handlers.rs` | HTTP endpoint handlers |
| `src/main.rs` | Self-registration logic |

## Version History

| Version | Changes |
|---------|---------|
| v1.12.0 | Initial Bazaar Discovery implementation |
