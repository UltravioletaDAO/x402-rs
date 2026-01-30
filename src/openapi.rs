//! OpenAPI/Swagger documentation for the x402 Facilitator API.
//!
//! This module provides interactive API documentation via Swagger UI at `/docs`.

use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use axum::Router;

/// OpenAPI documentation for the x402 Facilitator API
#[derive(OpenApi)]
#[openapi(
    info(
        title = "x402 Payment Facilitator API",
        version = "1.23.0",
        description = r#"
Ultravioleta DAO x402 Payment Facilitator - Gasless micropayments for the agentic economy.

## Overview

The x402 facilitator enables gasless micropayments across multiple blockchain networks using the HTTP 402 Payment Required protocol. It acts as a settlement intermediary, verifying EIP-3009/EIP-712 payment authorizations and submitting them on-chain.

## Supported Networks

### EVM Chains (Mainnet)
Ethereum, Base, Polygon, Optimism, Avalanche, Arbitrum, Celo, HyperEVM, Unichain, Monad, Scroll, BSC, SKALE

### EVM Chains (Testnet)
Ethereum Sepolia, Base Sepolia, Polygon Amoy, Optimism Sepolia, Avalanche Fuji, Arbitrum Sepolia, Celo Sepolia, HyperEVM Testnet, Unichain Sepolia, SKALE Sepolia

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

The facilitator supports [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) for AI agent reputation:

- `POST /feedback` - Submit on-chain reputation feedback
- `POST /feedback/revoke` - Revoke previously submitted feedback
- `POST /feedback/response` - Append agent response to feedback
- `GET /reputation/:network/:agentId` - Query agent reputation summary
- `GET /identity/:network/:agentId` - Get agent identity from registry

Supported ERC-8004 networks: `ethereum`, `ethereum-sepolia`

## Bazaar Discovery

Decentralized resource discovery for x402-enabled services:

- `GET /bazaar` - List all registered resources
- `POST /bazaar` - Register a new resource
- `GET /bazaar/:id` - Get specific resource by ID
- `DELETE /bazaar/:id` - Unregister a resource

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
        (name = "Core", description = "Core x402 payment verification and settlement"),
        (name = "Discovery", description = "Network and scheme discovery"),
        (name = "ERC-8004", description = "AI Agent reputation and identity (ERC-8004 Trustless Agents)"),
        (name = "Bazaar", description = "Decentralized resource discovery registry"),
        (name = "Health", description = "Service health and status")
    ),
    paths(
        // Core endpoints
        path_verify_get,
        path_verify_post,
        path_settle_get,
        path_settle_post,
        // Discovery endpoints
        path_supported,
        path_version,
        // ERC-8004 endpoints
        path_feedback_get,
        path_feedback_post,
        path_feedback_revoke,
        path_feedback_response,
        path_reputation,
        path_identity,
        // Bazaar endpoints
        path_bazaar_list,
        path_bazaar_register,
        path_bazaar_get,
        path_bazaar_delete,
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
      "network": "base-mainnet",
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
    "network": "base-mainnet",
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

**Request body:** Same as /verify

**Response on success:**
```json
{
  "success": true,
  "transaction": "0x...",
  "network": "base-mainnet",
  "payer": "0x..."
}
```

**Response on failure:**
```json
{
  "success": false,
  "errorReason": "insufficient_balance",
  "payer": "0x...",
  "network": "base-mainnet"
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
// Discovery Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/supported",
    tag = "Discovery",
    summary = "List supported payment kinds",
    description = r#"
Returns all supported payment kinds (network + scheme + version combinations).

**Response includes both v1 and v2 formats:**
- v1: `"network": "base-mainnet"` (string enum)
- v2: `"network": "eip155:8453"` (CAIP-2 format)
"#,
    responses(
        (status = 200, description = "Supported payment kinds", body = Object,
            example = json!({
                "kinds": [
                    {
                        "x402Version": 1,
                        "scheme": "exact",
                        "network": "base-mainnet"
                    },
                    {
                        "x402Version": 2,
                        "scheme": "exact",
                        "network": "eip155:8453"
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
    description = "Returns the current version of the facilitator.",
    responses(
        (status = 200, description = "Version info", body = Object,
            example = json!({
                "version": "1.23.0"
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
    path = "/feedback",
    tag = "ERC-8004",
    summary = "Get feedback submission schema",
    description = "Returns the JSON schema for ERC-8004 feedback submission requests.",
    responses(
        (status = 200, description = "Feedback schema", body = Object)
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

**Supported networks:** ethereum, ethereum-sepolia

**Request body:**
```json
{
  "x402Version": 1,
  "network": "ethereum",
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
      "network": "ethereum",
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
  "network": "ethereum"
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
  "network": "ethereum",
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
  "network": "ethereum",
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

**Supported networks:** ethereum, ethereum-sepolia

**Query parameters:**
- `include_feedback` (optional): Include individual feedback entries

**Response:**
```json
{
  "agentId": 42,
  "summary": {
    "agentId": 42,
    "count": 15,
    "summaryValue": 87,
    "summaryValueDecimals": 0,
    "network": "ethereum"
  },
  "feedback": [...],
  "network": "ethereum"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (ethereum or ethereum-sepolia)"),
        ("agent_id" = u64, Path, description = "Agent ID (ERC-721 tokenId)"),
        ("include_feedback" = Option<bool>, Query, description = "Include individual feedback entries")
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

**Supported networks:** ethereum, ethereum-sepolia

**Response:**
```json
{
  "agentId": 42,
  "owner": "0x...",
  "agentUri": "ipfs://Qm...",
  "agentWallet": "0x...",
  "network": "ethereum"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (ethereum or ethereum-sepolia)"),
        ("agent_id" = u64, Path, description = "Agent ID (ERC-721 tokenId)")
    ),
    responses(
        (status = 200, description = "Agent identity", body = Object),
        (status = 400, description = "Invalid network or agent", body = Object),
        (status = 404, description = "Agent not found", body = Object)
    )
)]
async fn path_identity() {}

// ============================================================================
// Bazaar Endpoints
// ============================================================================

#[utoipa::path(
    get,
    path = "/bazaar",
    tag = "Bazaar",
    summary = "List registered resources",
    description = r#"
Lists all resources registered in the Bazaar discovery registry.

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
      },
      "registered_at": "2024-01-01T00:00:00Z",
      "last_seen": "2024-01-02T00:00:00Z"
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
    path = "/bazaar",
    tag = "Bazaar",
    summary = "Register a resource",
    description = r#"
Registers a new resource in the Bazaar discovery registry.

**Request body:**
```json
{
  "url": "https://example.com/api",
  "type": "service",
  "description": "Example AI service",
  "paymentRequirements": [
    {
      "scheme": "exact",
      "network": "base-mainnet",
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

#[utoipa::path(
    get,
    path = "/bazaar/{id}",
    tag = "Bazaar",
    summary = "Get resource by ID",
    description = "Retrieves a specific resource from the Bazaar registry by its ID.",
    params(
        ("id" = String, Path, description = "Resource ID (UUID)")
    ),
    responses(
        (status = 200, description = "Resource details", body = Object),
        (status = 404, description = "Resource not found", body = Object)
    )
)]
async fn path_bazaar_get() {}

#[utoipa::path(
    delete,
    path = "/bazaar/{id}",
    tag = "Bazaar",
    summary = "Unregister a resource",
    description = "Removes a resource from the Bazaar registry.",
    params(
        ("id" = String, Path, description = "Resource ID (UUID)")
    ),
    responses(
        (status = 200, description = "Resource unregistered", body = Object),
        (status = 404, description = "Resource not found", body = Object)
    )
)]
async fn path_bazaar_delete() {}

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

/// Create the Swagger UI router
pub fn swagger_routes() -> Router {
    Router::new()
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
}
