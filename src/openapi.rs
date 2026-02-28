//! OpenAPI/Swagger documentation for the x402 Facilitator API.
//!
//! This module provides interactive API documentation via Swagger UI at `/docs`.
//!
//! **IMPORTANT**: Keep this file in sync with actual endpoints in `src/handlers.rs`.
//! When adding new endpoints or changing the version, update this file accordingly.
//! The version here should match `Cargo.toml` version.

use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// OpenAPI documentation for the x402 Facilitator API
#[derive(OpenApi)]
#[openapi(
    info(
        title = "x402 Payment Facilitator API",
        version = "0.0.0",  // Overridden at runtime by env!("CARGO_PKG_VERSION")
        description = r#"
Ultravioleta DAO x402 Payment Facilitator - Gasless micropayments for the agentic economy.

## Overview

The x402 facilitator enables gasless micropayments across multiple blockchain networks using the HTTP 402 Payment Required protocol. It acts as a settlement intermediary, verifying EIP-3009/EIP-712 payment authorizations and submitting them on-chain.

## Supported Networks

### EVM Chains (Mainnet)
Ethereum, Base, Polygon, Optimism, Avalanche, Arbitrum, Celo, HyperEVM, Unichain, Monad, Scroll, BSC, SKALE

### EVM Chains (Testnet)
Ethereum Sepolia, Base Sepolia, Polygon Amoy, Optimism Sepolia, Avalanche Fuji, Arbitrum Sepolia, Celo Sepolia, HyperEVM Testnet, Unichain Sepolia, SKALE Sepolia, Monad Testnet

### SVM Chains (Solana Virtual Machine)
- **Solana**: Mainnet and Devnet
- **Fogo**: Mainnet and Testnet

### Other Non-EVM Chains
- **NEAR Protocol**: Mainnet and Testnet
- **Stellar/Soroban**: Mainnet and Testnet
- **Algorand**: Mainnet and Testnet
- **Sui**: Mainnet and Testnet

## Core Endpoints

- `POST /verify` - Verify payment authorization structure and signatures
- `POST /settle` - Submit verified payment to blockchain for settlement
- `GET /supported` - List all supported networks and payment schemes

## ERC-8004 Reputation (Trustless Agents)

The facilitator supports [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) for AI agent identity and reputation across **14 networks** (8 mainnets + 6 testnets).

**Supported ERC-8004 networks:** `ethereum`, `base`, `polygon`, `arbitrum`, `celo`, `bsc`, `monad`, `avalanche`, `ethereum-sepolia`, `base-sepolia`, `polygon-amoy`, `arbitrum-sepolia`, `celo-sepolia`, `avalanche-fuji`

### Endpoints:
- `POST /register` - Register a new agent on-chain (gasless)
- `POST /feedback` - Submit on-chain reputation feedback
- `POST /feedback/revoke` - Revoke previously submitted feedback
- `POST /feedback/response` - Append agent response to feedback
- `GET /reputation/:network/:agentId` - Query agent reputation summary
- `GET /identity/:network/:agentId` - Get agent identity from registry
- `GET /identity/:network/:agentId/metadata/:key` - Read specific agent metadata
- `GET /identity/:network/total-supply` - Get total registered agents on a network

## Bazaar Discovery

Decentralized resource discovery for x402-enabled services:

- `GET /discovery/resources` - List all registered resources
- `POST /discovery/register` - Register a new resource

## Protocol Documentation

- [x402 Protocol](https://x402.org)
- [EIP-3009 (transferWithAuthorization)](https://eips.ethereum.org/EIPS/eip-3009)
- [ERC-8004 (Trustless Agents)](https://eips.ethereum.org/EIPS/eip-8004)
- [Ultravioleta DAO](https://ultravioletadao.xyz)
"#,
        contact(
            name = "Ultravioleta DAO",
            url = "https://ultravioletadao.xyz",
        ),
        license(
            name = "Apache-2.0",
            url = "https://www.apache.org/licenses/LICENSE-2.0"
        )
    ),
    servers(
        (url = "https://facilitator.ultravioletadao.xyz", description = "Production"),
        (url = "http://localhost:8080", description = "Local Development")
    ),
    tags(
        (name = "Core", description = "Core x402 payment verification and settlement (exact, upto, escrow schemes)"),
        (name = "Escrow", description = "Gasless escrow lifecycle (authorize, release, refund, state query)"),
        (name = "Discovery", description = "Network and scheme discovery"),
        (name = "ERC-8004", description = "AI Agent reputation and identity (ERC-8004 Trustless Agents) - 14 networks"),
        (name = "Bazaar", description = "Decentralized resource discovery registry"),
        (name = "Compliance", description = "OFAC compliance and sanctions screening"),
        (name = "Health", description = "Service health and status")
    ),
    paths(
        // Core endpoints
        path_verify_get,
        path_verify_post,
        path_settle_get,
        path_settle_post,
        // Escrow endpoints
        path_escrow_state,
        // Discovery endpoints
        path_supported,
        path_version,
        // ERC-8004 endpoints
        path_register_get,
        path_register_post,
        path_feedback_get,
        path_feedback_post,
        path_feedback_revoke,
        path_feedback_response,
        path_reputation,
        path_identity,
        path_identity_metadata,
        path_identity_total_supply,
        // Bazaar endpoints
        path_bazaar_list,
        path_bazaar_register,
        // Compliance
        path_blacklist,
        // Health
        path_health,
    )
)]
pub struct ApiDoc;

// ============================================================================
// Core Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/verify",
    tag = "Core",
    summary = "Get verification schema",
    description = "Returns the JSON schema for payment verification requests.",
    responses(
        (status = 200, description = "Verification schema", body = Object)
    )
)]
async fn path_verify_get() {}

#[utoipa::path(
    post,
    path = "/verify",
    tag = "Core",
    summary = "Verify payment authorization",
    description = r#"
Verifies an x402 payment authorization without settling it on-chain.

**Checks performed:**
- Payload structure validation
- EIP-712 signature verification
- Nonce validity
- Amount matching
- Timestamp validity (validAfter/validBefore)
- Token and network support

**Request body:**
```json
{
  "x402Version": 1,
  "paymentPayload": {
    "signature": "0x...",
    "payload": {
      "scheme": "exact",
      "network": "base",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "from": "0x...",
      "to": "0x...",
      "amount": "1000000",
      "validAfter": 1700000000,
      "validBefore": 1700100000,
      "nonce": "0x..."
    }
  },
  "paymentRequirements": {
    "scheme": "exact",
    "network": "base",
    "maxAmountRequired": "1000000",
    "payTo": "0x...",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
  }
}
```
"#,
    request_body(content = Object, description = "x402 verify request"),
    responses(
        (status = 200, description = "Verification result", body = Object,
            example = json!({
                "isValid": true
            })
        ),
        (status = 400, description = "Invalid request", body = Object,
            example = json!({
                "error": "Invalid signature"
            })
        )
    )
)]
async fn path_verify_post() {}

#[utoipa::path(
    get,
    path = "/settle",
    tag = "Core",
    summary = "Get settlement schema",
    description = "Returns the JSON schema for payment settlement requests.",
    responses(
        (status = 200, description = "Settlement schema", body = Object)
    )
)]
async fn path_settle_get() {}

#[utoipa::path(
    post,
    path = "/settle",
    tag = "Core",
    summary = "Settle payment on-chain",
    description = r#"
Submits a verified payment authorization to the blockchain for settlement.

**Process:**
1. Verifies the payment (same as /verify)
2. Calls `transferWithAuthorization` on the token contract
3. Returns transaction hash on success

**Upto Settlement (scheme: "upto"):**

When `scheme: "upto"`, the client provides a Permit2-signed authorization for a maximum amount.
The server settles for the actual usage amount (<= authorized max). If actual amount is 0, no
on-chain transaction is submitted.

Uses `x402UptoPermit2Proxy.settle(permit, amount, owner, witness, signature)` via Uniswap Permit2.

**Escrow Lifecycle (scheme: "escrow"):**

When `scheme: "escrow"` is set, the `action` field controls the operation:

| Action | Description | Signature Required |
|--------|-------------|-------------------|
| `authorize` (default) | Lock funds in escrow | Yes (ERC-3009) |
| `release` | Send escrowed funds to receiver | No |
| `refundInEscrow` | Return escrowed funds to payer | No |

Escrow contracts deployed on 9 networks. See `/supported` for networks with active PaymentOperator deployments.

**Escrow release/refund payload** (no signature needed):
```json
{
  "scheme": "escrow",
  "action": "release",
  "payload": {
    "paymentInfo": { "operator": "0x...", "receiver": "0x...", ... },
    "payer": "0x...",
    "amount": "1000000"
  },
  "paymentRequirements": {
    "network": "eip155:8453",
    "extra": { "escrowAddress": "0x...", "operatorAddress": "0x...", "tokenCollector": "0x..." }
  }
}
```

**Response on success:**
```json
{
  "success": true,
  "transaction": "0x...",
  "network": "base",
  "payer": "0x..."
}
```

**Response on failure:**
```json
{
  "success": false,
  "errorReason": "insufficient_balance",
  "payer": "0x...",
  "network": "base"
}
```
"#,
    request_body(content = Object, description = "x402 settle request"),
    responses(
        (status = 200, description = "Settlement result", body = Object),
        (status = 400, description = "Settlement failed", body = Object)
    )
)]
async fn path_settle_post() {}

// ============================================================================
// Escrow Endpoints
// ============================================================================

#[utoipa::path(
    post,
    path = "/escrow/state",
    tag = "Escrow",
    summary = "Query escrow payment state",
    description = r#"
Queries the on-chain state of an escrow payment from the AuthCaptureEscrow contract.
This is a read-only view call (no gas consumed).

Returns the capturable amount, refundable amount, and whether payment has been fully collected.

**Request body:**
```json
{
  "paymentInfo": {
    "operator": "0x...",
    "receiver": "0x...",
    "token": "0x...",
    "maxAmount": "1000000",
    "preApprovalExpiry": 281474976710655,
    "authorizationExpiry": 281474976710655,
    "refundExpiry": 281474976710655,
    "minFeeBps": 0,
    "maxFeeBps": 100,
    "feeReceiver": "0x...",
    "salt": "0x..."
  },
  "payer": "0x...",
  "network": "eip155:8453",
  "extra": {
    "escrowAddress": "0x...",
    "operatorAddress": "0x...",
    "tokenCollector": "0x..."
  }
}
```

**Response:**
```json
{
  "hasCollectedPayment": false,
  "capturableAmount": "1000000",
  "refundableAmount": "0",
  "paymentInfoHash": "0x...",
  "network": "eip155:8453"
}
```
"#,
    request_body(content = Object, description = "Escrow state query"),
    responses(
        (status = 200, description = "Escrow state", body = Object,
            example = json!({
                "hasCollectedPayment": false,
                "capturableAmount": "1000000",
                "refundableAmount": "0",
                "paymentInfoHash": "0xabcdef...",
                "network": "eip155:8453"
            })
        ),
        (status = 400, description = "Query failed", body = Object)
    )
)]
async fn path_escrow_state() {}

// ============================================================================
// Discovery Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/supported",
    tag = "Discovery",
    summary = "List supported payment kinds",
    description = r#"
Returns all supported payment kinds (network + scheme + version combinations).

**Schemes:**
- `exact` - Direct EIP-3009 payment settlement (v1 and v2 formats)
- `upto` - Permit2-based variable amount settlement (v2 only, CAIP-2 networks). Client authorizes a max amount; server settles actual usage (<= max). Ideal for usage-based pricing (LLM tokens, bandwidth, metered APIs).
- `escrow` - x402r PaymentOperator escrow (v2 only, CAIP-2 networks)
- `fhe_transfer` - FHE encrypted transfer via Zama (v1 and v2)

**Upto networks:** All EVM networks that support the `exact` scheme also support `upto` via the x402UptoPermit2Proxy contract (Permit2-based, CREATE2 address `0x4020633461b2895a48930Ff97eE8fCdE8E520002`).

**Escrow networks (9 total):** Base, Ethereum, Polygon, Arbitrum, Celo, Monad, Avalanche, Base Sepolia, Ethereum Sepolia.
Only networks with a deployed PaymentOperator appear in the response.

**Response includes both v1 and v2 formats:**
- v1: `"network": "base"` (string enum)
- v2: `"network": "eip155:8453"` (CAIP-2 format)
"#,
    responses(
        (status = 200, description = "Supported payment kinds", body = Object,
            example = json!({
                "kinds": [
                    {
                        "x402Version": 1,
                        "scheme": "exact",
                        "network": "base"
                    },
                    {
                        "x402Version": 2,
                        "scheme": "exact",
                        "network": "eip155:8453"
                    },
                    {
                        "x402Version": 2,
                        "scheme": "upto",
                        "network": "eip155:8453"
                    },
                    {
                        "x402Version": 2,
                        "scheme": "escrow",
                        "network": "eip155:8453",
                        "extra": {
                            "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                            "operatorAddress": "0x...",
                            "tokenCollector": "0x48ADf6E37F9b31dC2AAD0462C5862B5422C736B8"
                        }
                    }
                ]
            })
        )
    )
)]
async fn path_supported() {}

#[utoipa::path(
    get,
    path = "/version",
    tag = "Discovery",
    summary = "Get facilitator version",
    description = "Returns the current version of the facilitator. The version always matches the Cargo.toml package version.",
    responses(
        (status = 200, description = "Version info", body = Object,
            example = json!({
                "version": "(current Cargo.toml version)"
            })
        )
    )
)]
async fn path_version() {}

// ============================================================================
// ERC-8004 Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/register",
    tag = "ERC-8004",
    summary = "Get agent registration schema",
    description = "Returns the JSON schema for ERC-8004 agent registration requests, including supported networks and body format.",
    responses(
        (status = 200, description = "Registration schema", body = Object)
    )
)]
async fn path_register_get() {}

#[utoipa::path(
    post,
    path = "/register",
    tag = "ERC-8004",
    summary = "Register a new agent",
    description = r#"
Registers a new ERC-8004 agent on-chain. The facilitator pays all gas fees.

**Supported networks:** ethereum, base, polygon, arbitrum, celo, bsc, monad, avalanche, ethereum-sepolia, base-sepolia, polygon-amoy, arbitrum-sepolia, celo-sepolia, avalanche-fuji

If `recipient` is specified, the agent NFT is minted to the facilitator then transferred to the recipient via ERC-721 `safeTransferFrom`.

**Request body:**
```json
{
  "x402Version": 1,
  "network": "base",
  "agentUri": "ipfs://Qm.../agent.json",
  "metadata": [
    {"key": "description", "value": "0x..."},
    {"key": "website", "value": "0x..."}
  ],
  "recipient": "0x..."
}
```

**Response:**
```json
{
  "success": true,
  "agentId": 42,
  "transaction": "0x...",
  "transferTransaction": "0x...",
  "owner": "0x...",
  "network": "base"
}
```
"#,
    request_body(content = Object, description = "Agent registration request"),
    responses(
        (status = 200, description = "Registration result", body = Object),
        (status = 400, description = "Registration failed", body = Object)
    )
)]
async fn path_register_post() {}

#[utoipa::path(
    get,
    path = "/feedback",
    tag = "ERC-8004",
    summary = "Get feedback submission schema",
    description = "Returns the JSON schema for ERC-8004 feedback submission requests, including all supported networks and related endpoints.",
    responses(
        (status = 200, description = "Feedback schema with supported networks", body = Object)
    )
)]
async fn path_feedback_get() {}

#[utoipa::path(
    post,
    path = "/feedback",
    tag = "ERC-8004",
    summary = "Submit reputation feedback",
    description = r#"
Submits on-chain reputation feedback for an AI agent via the ERC-8004 Reputation Registry.

**Supported networks:** ethereum, base, polygon, arbitrum, celo, bsc, monad, avalanche, ethereum-sepolia, base-sepolia, polygon-amoy, arbitrum-sepolia, celo-sepolia, avalanche-fuji

**Request body:**
```json
{
  "x402Version": 1,
  "network": "base",
  "feedback": {
    "agentId": 42,
    "value": 87,
    "valueDecimals": 0,
    "tag1": "starred",
    "tag2": "quality",
    "endpoint": "https://agent.example/api",
    "feedbackUri": "ipfs://Qm...",
    "feedbackHash": "0x...",
    "proof": {
      "transactionHash": "0x...",
      "blockNumber": 12345678,
      "network": "base",
      "payer": "0x...",
      "payee": "0x...",
      "amount": "1000000",
      "token": "0x...",
      "timestamp": 1700000000,
      "paymentHash": "0x..."
    }
  }
}
```

**Response:**
```json
{
  "success": true,
  "transaction": "0x...",
  "feedbackIndex": 1,
  "network": "base"
}
```
"#,
    request_body(content = Object, description = "ERC-8004 feedback request"),
    responses(
        (status = 200, description = "Feedback submission result", body = Object),
        (status = 400, description = "Feedback submission failed", body = Object)
    )
)]
async fn path_feedback_post() {}

#[utoipa::path(
    post,
    path = "/feedback/revoke",
    tag = "ERC-8004",
    summary = "Revoke feedback",
    description = r#"
Revokes previously submitted reputation feedback.

**Request body:**
```json
{
  "x402Version": 1,
  "network": "base",
  "agentId": 42,
  "feedbackIndex": 1
}
```
"#,
    request_body(content = Object, description = "Revoke feedback request"),
    responses(
        (status = 200, description = "Revocation result", body = Object),
        (status = 400, description = "Revocation failed", body = Object)
    )
)]
async fn path_feedback_revoke() {}

#[utoipa::path(
    post,
    path = "/feedback/response",
    tag = "ERC-8004",
    summary = "Append response to feedback",
    description = r#"
Appends an agent's response to existing feedback.

**Request body:**
```json
{
  "x402Version": 1,
  "network": "base",
  "agentId": 42,
  "clientAddress": "0x...",
  "feedbackIndex": 1,
  "responseUri": "ipfs://Qm...",
  "responseHash": "0x..."
}
```
"#,
    request_body(content = Object, description = "Append response request"),
    responses(
        (status = 200, description = "Response appended", body = Object),
        (status = 400, description = "Failed to append response", body = Object)
    )
)]
async fn path_feedback_response() {}

#[utoipa::path(
    get,
    path = "/reputation/{network}/{agent_id}",
    tag = "ERC-8004",
    summary = "Get agent reputation",
    description = r#"
Queries the reputation summary for an AI agent from the ERC-8004 Reputation Registry.

**Supported networks:** ethereum, base, polygon, arbitrum, celo, bsc, monad, avalanche, ethereum-sepolia, base-sepolia, polygon-amoy, arbitrum-sepolia, celo-sepolia, avalanche-fuji

**Client address filtering:** The `clientAddresses` query parameter accepts comma-separated Ethereum addresses to filter reputation data by specific clients. If omitted, the endpoint auto-discovers all clients who have given feedback via the on-chain `getClients()` function.

**Examples:**
- `/reputation/base/42` - all clients (auto-discovered)
- `/reputation/base/42?clientAddresses=0xAAA,0xBBB` - specific clients only
- `/reputation/base/42?includeFeedback=true&tag1=quality` - with feedback entries filtered by tag

**Response:**
```json
{
  "agentId": 42,
  "summary": {
    "agentId": 42,
    "count": 15,
    "summaryValue": 87,
    "summaryValueDecimals": 0,
    "network": "base"
  },
  "feedback": [...],
  "network": "base"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base, polygon, arbitrum)"),
        ("agent_id" = u64, Path, description = "Agent ID (ERC-721 tokenId)"),
        ("include_feedback" = Option<bool>, Query, description = "Include individual feedback entries"),
        ("client_addresses" = Option<String>, Query, description = "Comma-separated client addresses to filter by. If omitted, auto-discovers all clients via getClients()")
    ),
    responses(
        (status = 200, description = "Reputation data", body = Object),
        (status = 400, description = "Invalid network or agent", body = Object),
        (status = 404, description = "Agent not found", body = Object)
    )
)]
async fn path_reputation() {}

#[utoipa::path(
    get,
    path = "/identity/{network}/{agent_id}",
    tag = "ERC-8004",
    summary = "Get agent identity",
    description = r#"
Retrieves agent identity information from the ERC-8004 Identity Registry.

**Supported networks:** ethereum, base, polygon, arbitrum, celo, bsc, monad, avalanche, ethereum-sepolia, base-sepolia, polygon-amoy, arbitrum-sepolia, celo-sepolia, avalanche-fuji

**Response:**
```json
{
  "agentId": 42,
  "owner": "0x...",
  "agentUri": "ipfs://Qm...",
  "agentWallet": "0x...",
  "network": "base"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base, polygon)"),
        ("agent_id" = u64, Path, description = "Agent ID (ERC-721 tokenId)")
    ),
    responses(
        (status = 200, description = "Agent identity", body = Object),
        (status = 400, description = "Invalid network or agent", body = Object),
        (status = 404, description = "Agent not found", body = Object)
    )
)]
async fn path_identity() {}

#[utoipa::path(
    get,
    path = "/identity/{network}/{agent_id}/metadata/{key}",
    tag = "ERC-8004",
    summary = "Read agent metadata",
    description = r#"
Reads a specific metadata key from an agent's Identity Registry entry.

**Response:**
```json
{
  "agentId": 42,
  "key": "description",
  "value": "0x48656c6c6f",
  "valueUtf8": "Hello",
  "network": "base"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base)"),
        ("agent_id" = u64, Path, description = "Agent ID (ERC-721 tokenId)"),
        ("key" = String, Path, description = "Metadata key (e.g., description, website, version)")
    ),
    responses(
        (status = 200, description = "Metadata value", body = Object),
        (status = 400, description = "Invalid network or agent", body = Object),
        (status = 404, description = "Agent or metadata key not found", body = Object)
    )
)]
async fn path_identity_metadata() {}

#[utoipa::path(
    get,
    path = "/identity/{network}/total-supply",
    tag = "ERC-8004",
    summary = "Get total registered agents",
    description = r#"
Returns the total number of registered agents on a specific network.

**Response:**
```json
{
  "network": "base",
  "totalSupply": 156
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base)")
    ),
    responses(
        (status = 200, description = "Total supply", body = Object),
        (status = 400, description = "Invalid or unsupported network", body = Object)
    )
)]
async fn path_identity_total_supply() {}

// ============================================================================
// Bazaar Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/discovery/resources",
    tag = "Bazaar",
    summary = "List registered resources",
    description = r#"
Lists all resources registered in the discovery registry.

**Query parameters:**
- `type` (optional): Filter by resource type (e.g., "facilitator", "agent", "service")
- `category` (optional): Filter by category
- `tag` (optional): Filter by tag

**Response:**
```json
{
  "resources": [
    {
      "id": "uuid-here",
      "url": "https://example.com/api",
      "type": "service",
      "description": "Example AI service",
      "paymentRequirements": [...],
      "metadata": {
        "category": "ai-agent",
        "provider": "Example Inc",
        "tags": ["ai", "nlp"]
      }
    }
  ]
}
```
"#,
    params(
        ("type" = Option<String>, Query, description = "Filter by resource type"),
        ("category" = Option<String>, Query, description = "Filter by category"),
        ("tag" = Option<String>, Query, description = "Filter by tag")
    ),
    responses(
        (status = 200, description = "List of resources", body = Object)
    )
)]
async fn path_bazaar_list() {}

#[utoipa::path(
    post,
    path = "/discovery/register",
    tag = "Bazaar",
    summary = "Register a resource",
    description = r#"
Registers a new resource in the discovery registry.

**Request body:**
```json
{
  "url": "https://example.com/api",
  "type": "service",
  "description": "Example AI service",
  "paymentRequirements": [
    {
      "scheme": "exact",
      "network": "base",
      "maxAmountRequired": "1000000",
      "payTo": "0x...",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
    }
  ],
  "metadata": {
    "category": "ai-agent",
    "provider": "Example Inc",
    "tags": ["ai", "nlp"]
  }
}
```
"#,
    request_body(content = Object, description = "Resource registration request"),
    responses(
        (status = 201, description = "Resource registered", body = Object),
        (status = 400, description = "Invalid request", body = Object)
    )
)]
async fn path_bazaar_register() {}

// ============================================================================
// Compliance Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/blacklist",
    tag = "Compliance",
    summary = "Get OFAC sanctioned addresses",
    description = r#"
Returns the list of OFAC sanctioned blockchain addresses. Payments involving these addresses are blocked.

**Response:**
```json
{
  "addresses": ["0x...", "0x..."],
  "lastUpdated": "2026-01-15T00:00:00Z",
  "source": "OFAC SDN List"
}
```
"#,
    responses(
        (status = 200, description = "Sanctioned addresses list", body = Object)
    )
)]
async fn path_blacklist() {}

// ============================================================================
// Health Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    summary = "Health check",
    description = "Returns the health status of the facilitator service.",
    responses(
        (status = 200, description = "Service is healthy", body = Object,
            example = json!({
                "status": "healthy"
            })
        )
    )
)]
async fn path_health() {}

/// Create the Swagger UI router.
///
/// The OpenAPI version is patched at compile time from `Cargo.toml` via `env!("CARGO_PKG_VERSION")`,
/// so it always stays in sync without manual updates.
pub fn swagger_routes() -> Router {
    let mut api_doc = ApiDoc::openapi();
    api_doc.info.version = env!("CARGO_PKG_VERSION").to_string();
    Router::new().merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", api_doc))
}
