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
Ethereum, Base, Polygon, Optimism, Avalanche, Arbitrum, Celo, HyperEVM, Unichain, Monad, Scroll, Robinhood Chain (settles Paxos USDG - no native USDC), BSC, SKALE Base

### EVM Chains (Testnet)
Ethereum Sepolia, Base Sepolia, Polygon Amoy, Optimism Sepolia, Avalanche Fuji, Arbitrum Sepolia, Celo Sepolia, HyperEVM Testnet, Unichain Sepolia, SKALE Base Sepolia, Monad Testnet, Robinhood Chain Testnet

### SVM Chains (Solana Virtual Machine)
- **Solana**: Mainnet (`solana`) and Devnet (`solana-devnet`)
- **Fogo**: Mainnet (`fogo`) and Testnet (`fogo-testnet`)
- **XRPL (XRP Ledger)**: Mainnet (`xrpl`) and Testnet (`xrpl-testnet`) — native XRP, pre-signed Payment transaction blobs; not the EVM `xrpl-evm` sidechain

### Other Non-EVM Chains
- **NEAR Protocol**: Mainnet (`near`) and Testnet (`near-testnet`)
- **Stellar/Soroban**: Mainnet (`stellar`) and Testnet (`stellar-testnet`)
- **Algorand**: Mainnet (`algorand`) and Testnet (`algorand-testnet`)
- **Sui**: Mainnet (`sui`) and Testnet (`sui-testnet`)

## Core Endpoints

- `POST /verify` - Verify payment authorization structure and signatures
- `POST /settle` - Submit verified payment to blockchain for settlement
- `GET /supported` - List all supported networks and payment schemes

## ERC-8004 Reputation (Trustless Agents)

The facilitator supports [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) for AI agent identity and reputation across **18 networks** (10 mainnets + 8 testnets), spanning both EVM and Solana.

**EVM networks:** `ethereum`, `base`, `polygon`, `arbitrum`, `optimism`, `celo`, `bsc`, `monad`, `avalanche`, `ethereum-sepolia`, `base-sepolia`, `polygon-amoy`, `arbitrum-sepolia`, `optimism-sepolia`, `celo-sepolia`, `avalanche-fuji`

**Solana networks:** `solana`, `solana-devnet` (via [QuantuLabs 8004-solana](https://github.com/QuantuLabs/8004-solana) + [ATOM Engine](https://github.com/QuantuLabs/8004-atom))

**Note:** For EVM networks, `agentId` is a numeric uint256 (e.g., `42`). For Solana, `agentId` is a base58 Pubkey (e.g., `7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv`). Solana reputation responses include bonus `atomStats` with trust tiers, quality scores, and anti-Sybil metrics.

### Endpoints:
- `POST /register` - Register a new agent on-chain (gasless; sync or async via `Prefer: respond-async`)
- `GET /register/status/{job_id}` - Poll an async registration until `agentId` is ready
- `POST /feedback` - Submit on-chain reputation feedback (EVM only)
- `POST /feedback/revoke` - Revoke previously submitted feedback (EVM only)
- `POST /feedback/response` - Append agent response to feedback (EVM only)
- `GET /reputation/:network/:agentId` - Query agent reputation summary (EVM + Solana)
- `GET /identity/:network/:agentId` - Get agent identity from registry (EVM + Solana)
- `GET /identity/:network/:agentId/metadata/:key` - Read specific agent metadata (EVM + Solana)
- `GET /identity/:network/total-supply` - Get total registered agents on a network (EVM + Solana)

## Bazaar Discovery

Curated resource discovery for x402-enabled services. Entries carry a discovery `source`,
a liveness `health` status from periodic probing, and a curated `tier`
(`first_party` > `vip` > `verified` > `listed`) which also drives listing order.

- `GET /discovery/resources` - List curated resources (filters: category, provider, tag, network, source, sourceFacilitator, q, health, tier; any other parameter is a 400)
- `GET /discovery/stats` - Aggregate catalog metrics (60s cache)
- `GET /bazaar` - HTML Bazaar explorer UI
- `GET /discovery/attestation/{hash}` - ERC-8004 attestation evidence body
- `POST /discovery/register` - Register a new resource (rate limited)

**Admin** (require `Authorization: Bearer <BAZAAR_ADMIN_TOKEN>`; return 404 when no admin token is configured):

- `DELETE /discovery/resources?url=...` - Permanently unregister a resource
- `POST /discovery/admin/suppress` - Hide a resource from listings without deleting it
- `POST /discovery/admin/release` - Un-suppress a resource

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
        (name = "ERC-8004", description = "AI Agent reputation and identity (ERC-8004 Trustless Agents) - 18 networks (EVM + Solana)"),
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
        path_accepts_post,
        // Escrow endpoints
        path_escrow_state,
        // Discovery endpoints
        path_supported,
        path_version,
        // ERC-8004 endpoints
        path_register_get,
        path_register_post,
        path_register_status,
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
        path_bazaar_stats,
        path_bazaar_ui,
        path_bazaar_attestation,
        path_bazaar_register,
        path_bazaar_admin_delete,
        path_bazaar_admin_suppress,
        path_bazaar_admin_release,
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

**Escrow / Commerce Lifecycle (scheme: "escrow" or "commerce"):**

Both `"escrow"` and `"commerce"` schemes are supported and functionally identical.
The `"commerce"` alias was introduced by x402r for marketplace integrations (e.g., Execution Market).
The `action` field controls the operation:

| Action | Description | Signature Required |
|--------|-------------|-------------------|
| `authorize` (default) | Lock funds in escrow | Yes (ERC-3009) |
| `release` | Send escrowed funds to receiver | No |
| `refundInEscrow` | Return escrowed funds to payer | No |

Escrow contracts deployed on 11 networks. See `/supported` for networks with active PaymentOperator deployments.

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

#[utoipa::path(
    post,
    path = "/accepts",
    tag = "Core",
    summary = "Negotiate payment requirements (Faremeter middleware)",
    description = r#"
Negotiation endpoint used by `@faremeter/middleware` and compatible x402 clients.

Receives merchant payment requirements, matches them against the facilitator's
supported capabilities, and returns enriched requirements with facilitator-specific
data (feePayer, tokens, escrow contracts, etc.).

**How it works:**
1. Middleware sends the merchant's desired payment requirements
2. Facilitator filters to only those it can handle (matching scheme + network)
3. Facilitator enriches each match with `extra` data (feePayer, tokens, features)
4. Middleware uses the enriched requirements in the 402 response to clients

**Supports both v1 and v2 network formats** (auto-detected):
- v1: `"network": "base"` (string enum)
- v2: `"network": "eip155:8453"` (CAIP-2 format)

**Request body:**
```json
{
  "x402Version": 1,
  "accepts": [
    {
      "scheme": "exact",
      "network": "solana",
      "maxAmountRequired": "10000",
      "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      "payTo": "SomePublicKey...",
      "description": "Access to premium API",
      "maxTimeoutSeconds": 90,
      "resource": "https://api.example.com/data"
    }
  ],
  "error": ""
}
```

**Response (enriched):**
```json
{
  "x402Version": 1,
  "accepts": [
    {
      "scheme": "exact",
      "network": "solana",
      "maxAmountRequired": "10000",
      "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      "payTo": "SomePublicKey...",
      "description": "Access to premium API",
      "maxTimeoutSeconds": 90,
      "resource": "https://api.example.com/data",
      "extra": {
        "feePayer": "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq",
        "tokens": [
          { "token": "usdc", "address": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "decimals": 6 }
        ]
      }
    }
  ],
  "error": ""
}
```

**Note:** Requirements for unsupported scheme+network combinations are silently dropped from the response.
"#,
    request_body(content = Object, description = "Merchant payment requirements to negotiate"),
    responses(
        (status = 200, description = "Enriched payment requirements", body = Object,
            example = json!({
                "x402Version": 1,
                "accepts": [
                    {
                        "scheme": "exact",
                        "network": "base",
                        "maxAmountRequired": "1000000",
                        "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                        "payTo": "0x...",
                        "extra": {
                            "tokens": [
                                { "token": "usdc", "address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "decimals": 6 }
                            ]
                        }
                    }
                ],
                "error": ""
            })
        ),
        (status = 400, description = "Invalid request", body = Object,
            example = json!({
                "x402Version": 1,
                "accepts": [],
                "error": "Missing or invalid 'accepts' array"
            })
        )
    )
)]
async fn path_accepts_post() {}

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

**Upto networks:** All EVM networks that support the `exact` scheme also support `upto` via the x402UptoPermit2Proxy contract (Permit2-based, canonical CREATE2 address `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002`, per the upstream x402 spec and @x402/evm SDK).

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

**Supported networks:** 18 networks (EVM + Solana). EVM chains use ERC-721 NFTs, Solana uses Metaplex Core NFTs.

**EVM request:**
```json
{
  "x402Version": 1,
  "network": "base",
  "agentUri": "ipfs://Qm.../agent.json",
  "metadata": [{"key": "description", "value": "0x..."}],
  "recipient": "0x..."
}
```

**Solana request:**
```json
{
  "x402Version": 1,
  "network": "solana",
  "agentUri": "ipfs://Qm.../agent.json",
  "metadata": [{"key": "x402Support", "value": "true"}]
}
```

**EVM response:** `agentId` is a numeric string (ERC-721 tokenId).
**Solana response:** `agentId` is a base58 Pubkey (Metaplex Core NFT mint address).

```json
{
  "success": true,
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "transaction": "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d6...",
  "owner": "facilitator-pubkey...",
  "network": "solana"
}
```

**Async mode (EVM):** send header `Prefer: respond-async` (or `X-Async: true`) to
get an immediate `202 Accepted` with a `jobId` instead of blocking on the ~28s
on-chain confirmation. Poll `GET /register/status/{jobId}` until `status` is
`done` and `agentId` is populated. The `Location` header of the 202 points at the
status URL, keeping the facilitator's on-chain latency out of the caller's
timeout budget.

**Idempotency / in-flight lock:** a second registration for the same
`network|agentUri|recipient` while the first is still confirming is not
re-minted — the async path returns the existing job, the sync path returns
`409 Conflict`.
"#,
    request_body(content = Object, description = "Agent registration request"),
    responses(
        (status = 200, description = "Registration result (sync)", body = Object),
        (status = 202, description = "Async registration accepted; poll /register/status/{jobId}", body = Object),
        (status = 400, description = "Registration failed", body = Object),
        (status = 409, description = "A registration for this agent is already in progress", body = Object)
    )
)]
async fn path_register_post() {}

#[utoipa::path(
    get,
    path = "/register/status/{job_id}",
    tag = "ERC-8004",
    summary = "Poll async registration status",
    description = r#"
Returns the status of an asynchronous ERC-8004 registration started with
`Prefer: respond-async` on `POST /register`.

`status` progresses `pending -> mint_confirmed -> done` (or `failed`). Once
`mint_confirmed`/`done`, `agentId` is populated. Terminal jobs are retained for
one hour before they age out (then this returns `404`).

```json
{
  "jobId": "reg_42",
  "status": "done",
  "network": "base",
  "agentId": "17",
  "transaction": "0x...",
  "transferTransaction": "0x...",
  "owner": "0x..."
}
```
"#,
    params(("job_id" = String, Path, description = "Job id from the async POST /register")),
    responses(
        (status = 200, description = "Current job status", body = Object),
        (status = 404, description = "Job not found or expired", body = Object)
    )
)]
async fn path_register_status() {}

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
Submits on-chain reputation feedback for an AI agent via the ERC-8004 Reputation Registry (EVM) or Agent Registry with ATOM Engine CPI (Solana).

**Supported networks:** 18 networks (EVM + Solana).

**agentId format:** Numeric (42) for EVM, base58 Pubkey string for Solana. Both JSON numbers and strings are accepted.

**EVM request:**
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
    "feedbackUri": "ipfs://Qm..."
  }
}
```

**Solana request:**
```json
{
  "x402Version": 1,
  "network": "solana",
  "feedback": {
    "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
    "value": 87,
    "valueDecimals": 0,
    "tag1": "quality",
    "tag2": "api",
    "endpoint": "https://agent.example/api",
    "feedbackUri": "ipfs://Qm..."
  }
}
```

Solana feedback triggers ATOM Engine CPI for trust scoring (trust tiers, HyperLogLog diversity, EMA quality).
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

**EVM request:**
```json
{
  "x402Version": 1,
  "network": "base",
  "agentId": 42,
  "feedbackIndex": 1
}
```

**Solana request** (requires `sealHash` for SEAL v1 integrity):
```json
{
  "x402Version": 1,
  "network": "solana",
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "feedbackIndex": 1,
  "sealHash": "0xabc123..."
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

**EVM request:**
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

**Solana request** (requires `sealHash` for SEAL v1 integrity, `clientAddress` as Solana Pubkey):
```json
{
  "x402Version": 1,
  "network": "solana",
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "clientAddress": "Bz5K7...",
  "feedbackIndex": 1,
  "responseUri": "ipfs://Qm...",
  "responseHash": "0x...",
  "sealHash": "0xabc123..."
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

**EVM networks:** ethereum, base, polygon, arbitrum, optimism, celo, bsc, monad, avalanche + testnets

**Solana networks:** solana, solana-devnet (reads from ATOM Engine for enriched reputation data)

**Client address filtering (EVM only):** The `clientAddresses` query parameter accepts comma-separated Ethereum addresses to filter reputation data by specific clients. If omitted, the endpoint auto-discovers all clients who have given feedback via the on-chain `getClients()` function.

**Examples:**
- `/reputation/base/42` - EVM agent (all clients, auto-discovered)
- `/reputation/solana/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv` - Solana agent (with ATOM stats)
- `/reputation/base/42?includeFeedback=true&tag1=quality` - with feedback entries filtered by tag

**EVM Response:**
```json
{
  "agentId": 42,
  "summary": { "count": 15, "summaryValue": 87, "summaryValueDecimals": 0 },
  "feedback": [...],
  "atomStats": null,
  "network": "base"
}
```

**Solana Response (includes ATOM Engine bonus data):**
```json
{
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "summary": { "count": 47, "summaryValue": 78, "summaryValueDecimals": 0 },
  "atomStats": {
    "trustTier": 3, "trustTierName": "Trusted",
    "qualityScore": 78, "confidence": 85, "riskScore": 12,
    "diversityRatio": 67, "positiveCount": 42, "negativeCount": 5
  },
  "network": "solana"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base, solana, solana-devnet)"),
        ("agent_id" = String, Path, description = "Agent ID: numeric for EVM (e.g., 42), base58 Pubkey for Solana"),
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

**EVM networks:** ethereum, base, polygon, arbitrum, optimism, celo, bsc, monad, avalanche + testnets

**Solana networks:** solana, solana-devnet (reads AgentAccount PDA from 8004-solana program)

**EVM Response:**
```json
{
  "agentId": 42,
  "owner": "0x...",
  "agentUri": "ipfs://Qm...",
  "agentWallet": "0x...",
  "network": "base"
}
```

**Solana Response:**
```json
{
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "owner": "5FHwkrdxPMsgAJBDkWmcoLiN9m1K95VCGw7qr4eXfjsP",
  "agentUri": "https://example.com/agent.json",
  "nftName": "My AI Agent",
  "feedbackCount": 47,
  "network": "solana"
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base, solana, solana-devnet)"),
        ("agent_id" = String, Path, description = "Agent ID: numeric for EVM (e.g., 42), base58 Pubkey for Solana")
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

Supports both EVM and Solana networks. On Solana, metadata is stored in MetadataEntryPda accounts derived from the agent's NFT address and metadata key hash.

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
        ("network" = String, Path, description = "Network name (e.g., ethereum, base, solana)"),
        ("agent_id" = String, Path, description = "Agent ID: numeric for EVM, base58 Pubkey for Solana"),
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

**EVM networks** return the ERC-721 totalSupply from the AgentRegistry contract.
**Solana networks** return the `baseIndex` from the RegistryConfig PDA (total minted agents).

**EVM Response:**
```json
{
  "network": "base",
  "totalSupply": 156
}
```

**Solana Response:**
```json
{
  "network": "solana",
  "totalSupply": 42
}
```
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., ethereum, base, solana)")
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
    summary = "List curated bazaar resources",
    description = r#"
Lists x402-enabled resources known to the curated Bazaar catalog.

**Ordering:** results are sorted by curated tier first (`first_party` > `vip` > `verified` > `listed`),
then by liveness (`alive` resources first), then by `lastUpdated` descending.

**Health visibility:** when `health` is omitted, quarantined resources are hidden.
Pass `health=any` to return everything, or a specific status to filter to it.

**Response:**
```json
{
  "x402Version": 2,
  "items": [
    {
      "url": "https://api.meshrelay.xyz/payments/access/alpha-test",
      "type": "http",
      "x402Version": 2,
      "description": "MeshRelay premium IRC channel access",
      "accepts": [
        {
          "scheme": "exact",
          "network": "eip155:8453",
          "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
          "amount": "100000",
          "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
          "maxTimeoutSeconds": 300
        }
      ],
      "lastUpdated": 1784818083,
      "metadata": {
        "provider": "MeshRelay",
        "category": "communication",
        "tags": ["irc"]
      },
      "source": "self_registered",
      "sourceFacilitator": null,
      "firstSeen": 1784818112,
      "health": {
        "status": "alive",
        "lastChecked": 1784900000,
        "httpStatus": 402,
        "latencyMs": 240
      },
      "curation": {
        "tier": "first_party",
        "label": "MeshRelay",
        "firstParty": true,
        "verification": {
          "protocol": "erc8004",
          "network": "base",
          "agentId": 2106,
          "feedbackCount": 0,
          "uptime": 99.77
        }
      }
    }
  ],
  "pagination": { "limit": 10, "offset": 0, "total": 21195 }
}
```

**Optional fields:** `metadata`, `sourceFacilitator`, `firstSeen`, `health` and `curation` are
omitted when unknown. Only `url`, `type`, `x402Version`, `accepts`, `lastUpdated` and `source`
are always present.

**Timestamps** (`firstSeen`, `lastUpdated`, `health.lastChecked`) are Unix epoch **seconds**,
serialized as JSON numbers -- not ISO-8601 strings and not milliseconds.

**Unknown parameters are rejected with a 400**, listing the ones supported. A parameter the
server accepted and ignored would be indistinguishable from a filter that matched everything,
so `?search=logs` fails loudly and points at `q` instead of quietly returning the whole catalog.
"#,
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of resources to return (default: 10, max: 100)"),
        ("offset" = Option<u32>, Query, description = "Number of resources to skip (default: 0)"),
        ("category" = Option<String>, Query, description = "Filter by metadata category (e.g., finance, communication)"),
        ("provider" = Option<String>, Query, description = "Filter by metadata provider name"),
        ("tag" = Option<String>, Query, description = "Filter by metadata tag"),
        ("network" = Option<String>, Query, description = "Exact CAIP-2 network match (e.g., eip155:8453, solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp)"),
        ("source" = Option<String>, Query, description = "Discovery source: self_registered | settlement | crawled | aggregated"),
        ("sourceFacilitator" = Option<String>, Query, description = "Facilitator the entry was aggregated from (e.g., coinbase, payai, thirdweb)"),
        ("q" = Option<String>, Query, description = "Free-text search over url, description, provider and tags. Max 128 characters (longer returns 400)"),
        ("health" = Option<String>, Query, description = "Liveness filter: alive | degraded | auth_gated | quarantined | unknown | unprobeable | any. When omitted, quarantined resources are hidden; 'any' returns everything"),
        ("tier" = Option<String>, Query, description = "Curated tier filter: first_party | vip | verified | listed")
    ),
    responses(
        (status = 200, description = "Curated resource listing", body = Object,
            example = json!({
                "x402Version": 2,
                "items": [{
                    "url": "https://api.meshrelay.xyz/payments/access/alpha-test",
                    "type": "http",
                    "x402Version": 2,
                    "description": "MeshRelay premium IRC channel access",
                    "accepts": [{
                        "scheme": "exact",
                        "network": "eip155:8453",
                        "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                        "amount": "100000",
                        "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
                        "maxTimeoutSeconds": 300
                    }],
                    "lastUpdated": 1784818083,
                    "metadata": {
                        "provider": "MeshRelay",
                        "category": "communication",
                        "tags": ["irc"]
                    },
                    "source": "self_registered",
                    "sourceFacilitator": null,
                    "firstSeen": 1784818112,
                    "health": {
                        "status": "alive",
                        "lastChecked": 1784900000,
                        "httpStatus": 402,
                        "latencyMs": 240
                    },
                    "curation": {
                        "tier": "first_party",
                        "label": "MeshRelay",
                        "firstParty": true,
                        "verification": {
                            "protocol": "erc8004",
                            "network": "base",
                            "agentId": 2106,
                            "feedbackCount": 0,
                            "uptime": 99.77
                        }
                    }
                }],
                "pagination": { "limit": 10, "offset": 0, "total": 21195 }
            })
        ),
        (status = 400, description = "Invalid query: an unsupported parameter, or `q` longer than 128 characters", body = Object,
            example = json!({
                "error": "unknown query parameter: search",
                "hint": "did you mean q?",
                "supported": [
                    "limit", "offset", "category", "network", "provider", "tag",
                    "source", "sourceFacilitator", "health", "tier", "q"
                ]
            })
        )
    )
)]
async fn path_bazaar_list() {}

#[utoipa::path(
    get,
    path = "/discovery/stats",
    tag = "Bazaar",
    summary = "Bazaar catalog statistics",
    description = r#"
Aggregate metrics for the curated Bazaar catalog. Served from a 60-second in-process cache,
so counters can lag recent registrations or health probes by up to a minute.

- `total` counts every resource in the catalog.
- `visible` counts the resources returned by the default `GET /discovery/resources` listing
  (quarantined resources excluded).

**Response:**
```json
{
  "total": 21195,
  "visible": 19263,
  "bySource": { "aggregated": 21067, "self_registered": 128 },
  "bySourceFacilitator": { "payai": 19800, "thirdweb": 622, "coinbase": 336 },
  "byNetwork": { "eip155:8453": 20991, "eip155:1": 56 },
  "byTier": { "first_party": 10, "vip": 127, "verified": 1814, "listed": 19244 },
  "byHealth": { "alive": 1814, "quarantined": 1932, "auth_gated": 263, "unknown": 17029 },
  "generatedAt": 1784900000
}
```
"#,
    responses(
        (status = 200, description = "Catalog metrics", body = Object,
            example = json!({
                "total": 21195,
                "visible": 19263,
                "bySource": { "aggregated": 21067, "self_registered": 128 },
                "bySourceFacilitator": { "payai": 19800, "thirdweb": 622, "coinbase": 336 },
                "byNetwork": { "eip155:8453": 20991, "eip155:1": 56 },
                "byTier": { "first_party": 10, "vip": 127, "verified": 1814, "listed": 19244 },
                "byHealth": { "alive": 1814, "quarantined": 1932, "auth_gated": 263, "unknown": 17029 },
                "generatedAt": 1784900000
            })
        )
    )
)]
async fn path_bazaar_stats() {}

#[utoipa::path(
    get,
    path = "/bazaar",
    tag = "Bazaar",
    summary = "Bazaar explorer UI",
    description = r#"
Serves the HTML Bazaar explorer: a browsable view of the curated catalog backed by
`GET /discovery/resources` and `GET /discovery/stats`.

Returns `text/html`, not JSON. Use the discovery endpoints for programmatic access.
"#,
    responses(
        (status = 200, description = "Bazaar explorer HTML page", content_type = "text/html", body = String)
    )
)]
async fn path_bazaar_ui() {}

#[utoipa::path(
    get,
    path = "/discovery/attestation/{hash}",
    tag = "Bazaar",
    summary = "Get attestation evidence",
    description = r#"
Serves the hosted ERC-8004 attestation evidence body referenced by an on-chain curation
attestation. The path key is a lowercase sha256 hex digest of the attested resource URL:
exactly 64 characters matching `[0-9a-f]{64}`. Any other shape is rejected with 400 so a URL
path segment can never be mapped to arbitrary content.

**Response (application/json):**
```json
{
  "type": "uptime",
  "endpoint": "https://mcp.execution.market/mcp",
  "network": "base",
  "agentId": 2106,
  "uptime": 99.77,
  "window": { "probes": 100, "ok": 99 },
  "prober": "uvd-bazaar-health/1.0"
}
```
"#,
    params(
        ("hash" = String, Path, description = "Lowercase sha256 hex digest of the resource URL (64 chars, [0-9a-f]{64})")
    ),
    responses(
        (status = 200, description = "Attestation evidence body", content_type = "application/json", body = Object,
            example = json!({
                "type": "uptime",
                "endpoint": "https://mcp.execution.market/mcp",
                "network": "base",
                "agentId": 2106,
                "uptime": 99.77,
                "window": { "probes": 100, "ok": 99 },
                "prober": "uvd-bazaar-health/1.0"
            })
        ),
        (status = 400, description = "Invalid evidence key format (not a 64-char lowercase hex digest)", body = String),
        (status = 404, description = "No evidence stored for that key", body = String)
    )
)]
async fn path_bazaar_attestation() {}

#[utoipa::path(
    post,
    path = "/discovery/register",
    tag = "Bazaar",
    summary = "Register a resource",
    description = r#"
Registers a paid resource in the Bazaar catalog so clients can discover it via
`GET /discovery/resources`.

**Rate limited** to roughly 5 requests per minute per IP: registration triggers DNS lookups and
outbound fetches against caller-supplied URLs.

**Request body:**
```json
{
  "url": "https://api.example.com/paid",
  "type": "http",
  "description": "Premium market data API",
  "accepts": [
    {
      "scheme": "exact",
      "network": "eip155:8453",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "amount": "10000",
      "payTo": "0x...",
      "maxTimeoutSeconds": 300
    }
  ],
  "metadata": {
    "provider": "Example",
    "category": "data",
    "tags": ["api"]
  }
}
```

**Fields:**
- `type` (required): one of `http`, `mcp`, `a2a`, `facilitator`.
- `accepts` (required, except for `facilitator` entries): payment options in x402 v2 shape with
  CAIP-2 `network` values.
- `metadata` (optional): `provider`, `category`, `tags`.

**Validation (400):** unsupported `scheme`, userinfo embedded in the URL
(`https://user:pass@host`), a host resolving to a private / loopback / link-local / cloud metadata
IP, or an empty `accepts` array on a non-`facilitator` type.
"#,
    request_body(content = Object, description = "Resource registration request"),
    responses(
        (status = 201, description = "Resource registered", body = Object,
            example = json!({
                "success": true,
                "message": "Resource registered successfully",
                "url": "https://api.example.com/paid"
            })
        ),
        (status = 400, description = "Validation failure (bad scheme, URL userinfo, private/metadata IP host, empty accepts)", body = Object),
        (status = 409, description = "Resource already registered", body = Object),
        (status = 429, description = "Rate limited (roughly 5 requests per minute per IP)", body = Object)
    )
)]
async fn path_bazaar_register() {}

#[utoipa::path(
    delete,
    path = "/discovery/resources",
    tag = "Bazaar",
    summary = "Unregister a resource (admin)",
    description = r#"
**Admin only.** Permanently removes a resource from the Bazaar catalog.

Requires an `Authorization: Bearer <BAZAAR_ADMIN_TOKEN>` header. When the server has no admin
token configured the whole admin surface is absent and this route returns **404**.

Rate limited to roughly 5 requests per minute per IP.

**Response:**
```json
{ "success": true, "url": "https://api.example.com/paid", "removed": true }
```
"#,
    params(
        ("url" = String, Query, description = "Exact resource URL to unregister"),
        ("Authorization" = String, Header, description = "Bearer <BAZAAR_ADMIN_TOKEN>")
    ),
    responses(
        (status = 200, description = "Resource removed", body = Object,
            example = json!({
                "success": true,
                "url": "https://api.example.com/paid",
                "removed": true
            })
        ),
        (status = 401, description = "Missing or invalid bearer token", body = Object),
        (status = 404, description = "Unknown URL, or admin surface disabled (no admin token configured)", body = Object)
    )
)]
async fn path_bazaar_admin_delete() {}

#[utoipa::path(
    post,
    path = "/discovery/admin/suppress",
    tag = "Bazaar",
    summary = "Suppress a resource (admin)",
    description = r#"
**Admin only.** Hides a resource from every listing without deleting it, so the entry can be
released later.

Requires an `Authorization: Bearer <BAZAAR_ADMIN_TOKEN>` header. When the server has no admin
token configured the whole admin surface is absent and this route returns **404**.

Rate limited to roughly 5 requests per minute per IP.

**Request body:**
```json
{ "url": "https://api.example.com/paid", "reason": "spam" }
```

**Response:**
```json
{ "success": true, "url": "https://api.example.com/paid", "suppressed": true }
```
"#,
    params(
        ("Authorization" = String, Header, description = "Bearer <BAZAAR_ADMIN_TOKEN>")
    ),
    request_body(content = Object, description = "Suppression request: url plus a reason string"),
    responses(
        (status = 200, description = "Resource suppressed", body = Object,
            example = json!({
                "success": true,
                "url": "https://api.example.com/paid",
                "suppressed": true
            })
        ),
        (status = 401, description = "Missing or invalid bearer token", body = Object),
        (status = 404, description = "Admin surface disabled (no admin token configured)", body = Object)
    )
)]
async fn path_bazaar_admin_suppress() {}

#[utoipa::path(
    post,
    path = "/discovery/admin/release",
    tag = "Bazaar",
    summary = "Release a suppressed resource (admin)",
    description = r#"
**Admin only.** Reverses `POST /discovery/admin/suppress`, making the resource visible in
listings again.

Requires an `Authorization: Bearer <BAZAAR_ADMIN_TOKEN>` header. When the server has no admin
token configured the whole admin surface is absent and this route returns **404**.

Rate limited to roughly 5 requests per minute per IP.

**Request body:**
```json
{ "url": "https://api.example.com/paid" }
```

**Response:**
```json
{ "success": true, "url": "https://api.example.com/paid", "suppressed": false }
```
"#,
    params(
        ("Authorization" = String, Header, description = "Bearer <BAZAAR_ADMIN_TOKEN>")
    ),
    request_body(content = Object, description = "Release request: url of the suppressed resource"),
    responses(
        (status = 200, description = "Resource released", body = Object,
            example = json!({
                "success": true,
                "url": "https://api.example.com/paid",
                "suppressed": false
            })
        ),
        (status = 401, description = "Missing or invalid bearer token", body = Object),
        (status = 404, description = "Admin surface disabled (no admin token configured)", body = Object)
    )
)]
async fn path_bazaar_admin_release() {}

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
