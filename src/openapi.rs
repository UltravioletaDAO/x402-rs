//! OpenAPI/Swagger documentation for the x402 Facilitator API.
//!
//! This module provides interactive API documentation via Swagger UI at `/docs`.
//!
//! **IMPORTANT**: Keep this file in sync with actual endpoints in `src/handlers.rs`.
//! When adding new endpoints, update this file accordingly. The version needs no
//! attention: it is patched at runtime from the `VERSION` file via
//! `FACILITATOR_VERSION` (see `src/version.rs`), and the literal below is a
//! placeholder that is always overridden.

use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// OpenAPI documentation for the x402 Facilitator API
#[derive(OpenApi)]
#[openapi(
    info(
        title = "x402 Payment Facilitator API",
        version = "0.0.0",  // Overridden at runtime by crate::version::facilitator_version()
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

The facilitator supports [ERC-8004](https://eips.ethereum.org/EIPS/eip-8004) for AI agent identity and reputation across **21 networks** (12 mainnets + 9 testnets), spanning both EVM and Solana.

**EVM networks:** `ethereum`, `base`, `polygon`, `arbitrum`, `optimism`, `celo`, `bsc`, `monad`, `avalanche`, `ethereum-sepolia`, `base-sepolia`, `polygon-amoy`, `arbitrum-sepolia`, `optimism-sepolia`, `celo-sepolia`, `avalanche-fuji`

**Solana networks:** `solana`, `solana-devnet` (via [QuantuLabs 8004-solana](https://github.com/QuantuLabs/8004-solana) + [ATOM Engine](https://github.com/QuantuLabs/8004-atom))

**Note:** For EVM networks, `agentId` is a numeric uint256 (e.g., `42`). For Solana, `agentId` is a base58 Pubkey (e.g., `7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv`). Solana reputation responses include bonus `atomStats` with trust tiers, quality scores, and anti-Sybil metrics.

### Endpoints:
- `POST /register` - Register a new agent on-chain (gasless; sync or async via `Prefer: respond-async`)
- `GET /register/status/{job_id}` - Poll an async registration until `agentId` is ready
- `POST /feedback` - Submit on-chain reputation feedback (EVM only)
- `POST /feedback/revoke` - Revoke previously submitted feedback (EVM only). **Admin only**: requires `Authorization: Bearer <ERC8004_ADMIN_TOKEN>` and returns 404 when no token is configured
- `POST /feedback/evm/prepare` - Build the digest the RATER signs, for an EIP-7702 relayed rating (EVM)
- `POST /feedback/evm/submit` - Relay a rater-authored rating as a type-4 transaction, paying the gas (EVM)
- `POST /feedback/solana/prepare` - Build a feedback transaction for the RATER to sign (Solana)
- `POST /feedback/solana/submit` - Co-sign and send a rater-signed feedback transaction (Solana)
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

## Errors

Every refusal is JSON with `content-type: application/json`, including the ones
a framework normally answers with an empty body: a `405` for the wrong method,
a `404` for a path nothing serves, and the `429` from the rate limiter. The
shape is:

```json
{
  "error": "human-readable, may change",
  "code": "machine_readable_stable",
  "hint": "what to do about it"
}
```

Branch on `code`, never on the prose in `error`. Codes in use include
`invalid_request_body`, `method_not_allowed`, `not_found`, `not_acceptable`,
`rate_limited` and `rate_limit_key_unavailable`; endpoint-specific codes are
documented on the operations that return them.

## Rate limits

Limits are per client IP and are reported on every rate-limited response, not
just on the refusal:

| Header | Present on | Meaning |
|---|---|---|
| `x-ratelimit-limit` | `200` and `429` | burst size of the bucket this route draws on |
| `x-ratelimit-remaining` | `200` and `429` | tokens left in that bucket |
| `retry-after` | `429` | seconds to wait before retrying |
| `x-ratelimit-after` | `429` | the same value under tower_governor's own name |

Read `x-ratelimit-remaining` and slow down before it reaches zero. The buckets
refill one token every N seconds rather than granting N per minute, so the
sustained rate and the burst are different numbers. Surfaces have separate
buckets, with one deliberate exception: `POST /mcp` shares the `/verify` and
`/settle` bucket, because an `x402_settle` tool call costs the chain exactly
what `POST /settle` does. Free static routes (`/health`, `/supported`, the
discovery documents) carry no limit and therefore no headers.

## Content negotiation

`GET /` answers `text/html` by default and `text/markdown` -- the bytes of
`/index.md` -- to a request whose `Accept` prefers it. `/llms.txt` relabels its
own bytes the same way. Those responses carry `Vary: Accept, Accept-Encoding`.
Negotiation follows RFC 9110 12.5.1: ranked by `q`, ties broken by specificity,
`q=0` honoured as a refusal, and a missing `Accept` or `*/*` treated as no
constraint rather than as grounds for a `406`.

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
        (name = "ERC-8004", description = "AI Agent reputation and identity (ERC-8004 Trustless Agents) - 21 networks (EVM + Solana)"),
        (name = "Bazaar", description = "Decentralized resource discovery registry"),
        (name = "Compliance", description = "OFAC compliance and sanctions screening"),
        (name = "Health", description = "Service health and status"),
        (name = "Agentic", description = "Machine-readable discovery surfaces (llms.txt, A2A card, x402 discovery, RFC 9727 catalog, skills index, MCP server card)"),
        (name = "MCP", description = "Model Context Protocol server (Streamable HTTP, stateless) exposing verify/settle/supported/accepts as tools")
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
        path_events,
        path_transactions,
        path_api_stats,
        // ERC-8004 endpoints
        path_register_get,
        path_register_post,
        path_register_status,
        path_feedback_get,
        path_feedback_post,
        path_feedback_evm_prepare,
        path_feedback_evm_submit,
        path_feedback_solana_prepare,
        path_feedback_solana_submit,
        path_feedback_revoke,
        path_feedback_response,
        path_reputation,
        path_identity,
        path_identity_by_owner,
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
        // DX402 durable-evidence
        path_dx402_anchor,
        path_dx402_evidence,
        path_dx402_receipt,
        path_dx402_stats,
        path_dx402_blob,
        path_dx402_recover,
        path_dx402_repair,
        // Compliance
        path_blacklist,
        // Health
        path_health,
        // Agentic discovery surfaces
        path_llms_txt,
        path_llms_full_txt,
        path_robots_txt,
        path_sitemap_xml,
        path_index_md,
        path_skill_md,
        path_auth_md,
        path_workflows_json,
        path_openapi_json,
        path_agent_card,
        path_agent_json_legacy,
        path_x402_discovery,
        path_api_catalog,
        path_oauth_protected_resource,
        path_agent_skills_index,
        path_mcp_server_card,
        // MCP
        path_mcp_post,
        path_mcp_get,
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

**Upto networks (11):** Base, Optimism, Arbitrum, Polygon, BSC, Ethereum, HyperEVM, Monad, Base Sepolia, Avalanche Fuji, Arbitrum Sepolia — via the x402UptoPermit2Proxy contract (Permit2-based, canonical CREATE2 address `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002`).

`upto` is **not** available on every EVM network that supports `exact`. The proxy address is identical on all chains because it is deployed with CREATE2, but the deployment still has to be replayed per chain, and on Avalanche, Celo, Scroll, Unichain and Optimism Sepolia it never was — the address has no code there. Query `/supported` rather than assuming: it now lists `upto` only where settlement can actually succeed.

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

#[utoipa::path(
    get,
    path = "/events",
    tag = "Discovery",
    summary = "Live traffic stream (Server-Sent Events)",
    description = "Streams one Server-Sent Event per facilitator operation, so observers can \
render live traffic without scraping logs. The SSE `event:` name is the operation \
(`verify` or `settle`) and `data:` is the JSON payload below; a `:keepalive` comment every \
15s holds the connection open through the load balancer.\n\n\
`network` is the facilitator's canonical slug, the same one `/supported` uses. A `settle` \
carries the transaction hash; a `verify` has none, because nothing has settled yet.\n\n\
The stream is lossy on purpose: it can never slow down or fail a payment, so a subscriber \
that falls behind loses events and stays connected. It is also bounded — the endpoint \
returns **503** with `Retry-After` once the concurrent-subscriber cap is reached, and **404** \
when the operator has disabled it with `X402_EVENTS_ENABLED=false`.\n\n\
Operators can narrow what is published without a code change: `X402_EVENTS_DETAIL=minimal` \
drops `payer`/`tx`/`amount`/`asset`, and `X402_EVENTS_SCOPE=allowlist` restricts the stream \
to payers in `X402_EVENTS_ALLOWLIST`.\n\n\
By default operations that ERROR are not published, so `ok:false` only ever means \
\"resolved and came back negative\". Set `X402_EVENTS_PUBLISH_FAILURES=true` to emit them; \
they carry an `error` field holding a bounded CATEGORY (`contract_revert`, \
`invalid_signature`, `insufficient_funds`, …) and never the error text.",
    responses(
        (status = 200, description = "SSE stream of traffic events (`text/event-stream`)", body = Object,
            example = json!({
                "ts": 1769000000000_u64,
                "kind": "settle",
                "network": "base",
                "ok": true,
                "payer": "0x...",
                "tx": "0x...",
                "amount": "20000",
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "resource": "https://api.example.com/premium-data",
                "payTo": "0x...",
                "description": "Premium data feed",
                "scheme": "exact"
            })
        ),
        (status = 404, description = "Stream disabled by the operator (X402_EVENTS_ENABLED=false)"),
        (status = 503, description = "Concurrent-subscriber cap reached; retry after the `Retry-After` delay")
    )
)]
async fn path_events() {}

#[utoipa::path(
    get,
    path = "/transactions",
    tag = "Discovery",
    summary = "Recent operations the facilitator recorded",
    description = "Recent verify/settle operations, newest first.\n\n**This is an index, not a ledger.** The record is written best-effort AFTER the operation resolved, so an unreachable store loses rows and never blocks a payment. The chain is authoritative; a row missing here does not mean the payment did not happen.\n\nCounting starts when the store was enabled — earlier operations are absent, not zero. `limit` is capped at 200.",
    params(
        ("limit" = Option<usize>, Query, description = "Rows to return (1-200, default 50)"),
        ("network" = Option<String>, Query, description = "Canonical slug, e.g. `base`")
    ),
    responses(
        (status = 200, description = "Recent operations", body = Object),
        (status = 503, description = "Transaction store unavailable or not configured")
    )
)]
async fn path_transactions() {}

#[utoipa::path(
    get,
    path = "/api/stats",
    tag = "Discovery",
    summary = "Pre-aggregated totals per network and asset",
    description = "Totals maintained on write, so the cost of this call does not grow with history — it never scans.\n\nTwo caveats travel in the response body because they change how the numbers should be read. **Operations that ERROR are not recorded at all**, so a 100% success rate means 'no failures were recorded', not 'no failures occurred'. And **counting began when the store was enabled**, so anything settled before that is unknown rather than zero.\n\n`volumeAtomic` is a STRING: these are u256-shaped values and a JSON number silently loses precision above 2^53.",
    responses(
        (status = 200, description = "Aggregated totals", body = Object),
        (status = 503, description = "Store unavailable or not configured")
    )
)]
async fn path_api_stats() {}

// ============================================================================
// DX402 durable-evidence
// ============================================================================

#[utoipa::path(
    post,
    path = "/dx402/anchor",
    tag = "DX402",
    summary = "Register sealed evidence for a settled payment",
    description = "A resource server reports that it sealed a response body and wrote the ciphertext somewhere durable. The facilitator notarises the claim with an EIP-712 `EvidenceReceipt` and indexes it.\n\n**This request carries metadata only.** The plaintext never reaches the facilitator, and in `direct` mode neither does anything that could decrypt it — the content key is wrapped to the payer's own public key, recovered from the payment signature. A leak of this facilitator's storage reveals pointers and hashes, never payloads.\n\nAvailable only when `ENABLE_DX402=true`; otherwise the route does not exist.",
    responses(
        (status = 201, description = "Evidence recorded; body carries the signed receipt", body = Object),
        (status = 503, description = "Store or index unavailable — RETRYABLE, do not record as 'no evidence'")
    )
)]
async fn path_dx402_anchor() {}

#[utoipa::path(
    get,
    path = "/dx402/evidence/{paymentId}",
    tag = "DX402",
    summary = "Look up the evidence anchored for a payment",
    description = "Returns the pointer, the plaintext content hash, the mode, and the signed receipt.\n\n`contentHash` is over the **plaintext**, deliberately. Hashing the ciphertext would only prove the blob was not corrupted in storage; hashing the plaintext lets a buyer prove the anchored blob decrypts to exactly the bytes they were served — the check that catches a seller anchoring something other than what it delivered.\n\n**404 and 410 are different answers.** 404 means no evidence was ever recorded; 410 means the retention window lapsed. In a dispute those are not interchangeable.",
    params(("paymentId" = String, Path, description = "keccak256(caip2Network || txHash), or the `payment-identifier` value")),
    responses(
        (status = 200, description = "Evidence record", body = Object),
        (status = 404, description = "No evidence recorded for this payment"),
        (status = 410, description = "Past the retention window"),
        (status = 503, description = "Index unavailable — RETRYABLE")
    )
)]
async fn path_dx402_evidence() {}

#[utoipa::path(
    get,
    path = "/dx402/receipt/{paymentId}",
    tag = "DX402",
    summary = "The signed evidence receipt, verifiable offline",
    description = "The EIP-712 receipt alone, plus the domain and the signer address.\n\nAnyone can verify this without calling the facilitator again — which is precisely the property the IETF x402 receipt drafts identify as missing from a bare `PAYMENT-RESPONSE`, where an auditor cannot validate a retained receipt without contacting the facilitator.\n\nDomain: `{name: \"DX402 Evidence\", version: \"1\", chainId}`. The receipt records `mode`, because a `direct` receipt and an `escrowed` receipt make materially different claims about who can read the payload.",
    params(("paymentId" = String, Path, description = "Payment identifier")),
    responses(
        (status = 200, description = "Signed receipt with its domain and signer", body = Object),
        (status = 404, description = "No evidence recorded"),
        (status = 410, description = "Past the retention window")
    )
)]
async fn path_dx402_receipt() {}

#[utoipa::path(
    get,
    path = "/dx402/stats",
    tag = "DX402",
    summary = "How much evidence this facilitator has notarised",
    description = "Anchor count, configured backend and retention, and the address that signs receipts.\n\n`anchored` is a **floor**, not a ledger. Evidence whose index write failed is real and is not counted here, in the same way `/api/stats` undercounts operations.",
    responses((status = 200, description = "DX402 status and counters", body = Object))
)]
async fn path_dx402_stats() {}

#[utoipa::path(
    get,
    path = "/dx402/blob/{paymentId}",
    tag = "DX402",
    summary = "The sealed ciphertext for a payment",
    description = "Streams the sealed evidence blob. **Unauthenticated on purpose.**\n\nIn `direct` mode the bytes are sealed to the payer's own public key, so handing them to anyone who asks reveals nothing — the access control lives in the cryptography rather than in an ACL that could be misconfigured. This is also why the evidence bucket is private and never exposed: this route is the only way in, and it can only ever serve ciphertext.\n\nA DX402 pointer (`s3+https://…/dx402/blob/{paymentId}`) resolves here. Pointers address the *payment*, not the storage layout, so one a buyer is holding a year from now keeps working even if the backing keys are reorganised.\n\nResponses are cacheable for the same reason they are public: an intermediary that caches this stores something unreadable. Attack III of *Five Attacks on x402* measured 100% leakage of paid responses through an nginx cache; DX402 makes that leak worthless.",
    params(("paymentId" = String, Path, description = "Payment identifier")),
    responses(
        (status = 200, description = "Sealed ciphertext (application/octet-stream)", body = String),
        (status = 404, description = "No evidence recorded"),
        (status = 410, description = "Past the retention window"),
        (status = 503, description = "Store unavailable — RETRYABLE")
    )
)]
async fn path_dx402_blob() {}

#[utoipa::path(
    post,
    path = "/dx402/recover",
    tag = "DX402",
    summary = "Release a wrapped content key (escrowed mode)",
    description = "**Returns 501 in v0.1.**\n\n`direct` mode — the default and the whole point of DX402 — needs no recovery endpoint at all: the buyer already holds the only key that opens the payload, so retrieval is arithmetic rather than an authorization decision anyone could refuse or misconfigure.\n\nThis returns an honest 501 rather than a stub that appears to work, so no integrator builds an escrowed flow against a signature check that does not exist yet.",
    responses((status = 501, description = "Escrowed mode is not implemented in v0.1"))
)]
async fn path_dx402_recover() {}

#[utoipa::path(
    post,
    path = "/dx402/repair/{paymentId}",
    tag = "DX402",
    summary = "Audit one anchor, and optionally correct a pointer that names nothing",
    description = "**Admin only.** `Authorization: Bearer <DX402_ADMIN_TOKEN>`, and **404 when no token is configured** — fail-closed, so the route is indistinguishable from absent. Its own token, deliberately not shared with the bazaar or ERC-8004 admin surfaces: this one re-signs a facilitator attestation.\n\nExists for the anchors written while the evidence pointer was a *prediction* nobody reconciled. On the `ipfs` backend a Pinata failure put the bytes in the S3 fallback while the record — and the signed receipt — went on naming an IPFS object that never existed. Reading it fails silently: the fallback store treats the primary's `NotFound` as a verdict and never retries, so the anchor returned 201, the receipt carries our signature, and the evidence is unreachable with no error anywhere.\n\n`write` defaults to **false**. An audit reports `repairable` and changes nothing; only `?write=true` rewrites. Auditing is safe and rewriting a signed attestation is not, so the dangerous half has to be asked for by name — otherwise the safe-looking call would be the dangerous one.\n\nA repair re-signs, because `pointer` is part of the EIP-712 type hash and a corrected pointer under the old signature is a receipt that does not verify. That is why this lives here rather than in a script: the signing key must not leave the service. It can only ever change *where the bytes are* — `verified` and `signed` are carried structurally from the record it read, so a repair cannot escalate authority.\n\n`lost` is never papered over. A record pointing at a real absence is telling the truth, and rewriting it would only hide that the evidence is gone.",
    params(
        ("paymentId" = String, Path, description = "keccak256(caip2Network || txHash)"),
        ("write" = Option<bool>, Query, description = "false (default) audits; true rewrites the record and re-signs")
    ),
    responses(
        (status = 200, description = "Verdict: healthy | repairable | repaired | lost"),
        (status = 401, description = "Missing or invalid admin credentials"),
        (status = 404, description = "No admin token configured, or the payment has no evidence"),
        (status = 409, description = "The row changed between the audit and the write; nothing was touched")
    )
)]
async fn path_dx402_repair() {}

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

**Supported networks:** 21 networks (EVM + Solana). EVM chains use ERC-721 NFTs, Solana uses Metaplex Core NFTs.

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

**Solana request:** `recipient` is a base58 Solana address.
```json
{
  "x402Version": 1,
  "network": "solana",
  "agentUri": "ipfs://Qm.../agent.json",
  "metadata": [{"key": "x402Support", "value": "true"}],
  "recipient": "6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv"
}
```

On Solana the facilitator mints, initializes the agent's ATOM stats account, then
transfers the Metaplex Core asset to `recipient`, paying every fee. The ordering is
required: only the owner can initialize the stats, so it happens before the transfer.
Without that account the ATOM Engine records feedback but scores none of it.

`agentWallet` does not survive the transfer and must be re-set by the new owner,
the same as on EVM.

If the mint succeeds but the transfer fails, the response is a 500 that still carries
`agentId` and `transaction`: the agent exists and is held by the facilitator, and is
never reported as delivered.

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

**Supported networks:** 21 networks (EVM + Solana).

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
    "feedbackUri": "ipfs://Qm...",
    "score": 95
  }
}
```

**Proof of payment (anti-sybil gate).** The Reputation Registry lets any address rate any agent, so
a `proof` (the `ProofOfPayment` the settle response already returns) plus a `rater` address turn a
rating into something backed by a real payment. When both are present the facilitator checks, on-chain
and server-side: the transaction exists on that network and succeeded, sits in the block the proof
claims, contains an ERC-20 `Transfer` of exactly `amount` in `token` from `payer` to `payee`, the payer
is the `rater`, the payee is an address the Identity Registry ties to the agent (`getAgentWallet` when
set, otherwise `ownerOf`), the block timestamp is within `ERC8004_PROOF_MAX_AGE_SECS`, `paymentHash`
recomputes, and this (payment, agent) pair has not already been spent on a rating.

The verdict comes back in the response `proof` field with a bounded reason. While
`ERC8004_REQUIRE_PROOF` is off, a failing proof is reported but the feedback is still written; with it
on, the submission is rejected with 400. Two verdicts never block a write: `proof_rpc_unavailable`
(no verdict reached - our outage must not erase reputation) and `proof_unverifiable_chain` (a Solana
feedback, where the payment half of the gate has no EVM receipt to read).

If a `feedbackUri` + `feedbackHash` pair is supplied, the facilitator also fetches the document and
verifies `keccak256(document) == feedbackHash`, reporting whether the anchor is `auditable`,
`hash_only`, `unreachable` or a `mismatch`. A mismatch is refused: the hash would commit to something
other than what was shown.

Solana feedback triggers ATOM Engine CPI for trust scoring (trust tiers, HyperLogLog diversity, EMA quality).

**Send `score` (0-100) on Solana.** Without it the engine records the feedback on the
agent but scores nothing - the program reports `had_impact=false` and reputation stays
at zero no matter how much feedback accumulates.
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
    path = "/feedback/evm/prepare",
    tag = "ERC-8004",
    summary = "Prepare a rater-authored feedback (EVM, EIP-7702)",
    description = r#"
Returns everything the **rater** must sign so that the chain records THEM as the author of the
rating while the facilitator pays the gas.

**Why this exists.** The Reputation Registry records `msg.sender` as the author and the deployed
implementation has no delegation path at all - no `giveFeedbackWithSignature`, no ERC-2771
forwarder. A rating the facilitator relays normally is a rating attributed to the *facilitator*:
87,2% of the feedback on Base, and the same address can revoke any of it.

EIP-7702 fixes it without touching the registry. The rater delegates their own EOA to Execution
Market's `FeedbackDelegate`, and the facilitator sends the transaction **to the rater's own
address**, so the registry observes the rater as `msg.sender`.

`rater` is REQUIRED (EVM address). The response carries:

- `delegate` - the FeedbackDelegate the account must be pointed at
- `data` - the registry calldata being authorised
- `digest` - the value the signature must recover against. **The EIP-191 envelope is already
  applied.** Sign it RAW, as a prehash (`unsafe_sign_hash`, `signHash`, `sign_hash_sync`)
- `typedData` - **present only on a v4 delegate**, and when it is there, sign IT. The full
  `eth_signTypedData_v4` payload: the wallet renders the agent, the score, the tags and the
  deadline as named fields, so the rater sees what they authorise instead of a hex blob. v4
  carries no `signingPayload` and needs none — `signTypedData` has no envelope to apply twice
- `signingPayload` - **v3 delegates only.** The same hash with the envelope still OFF, and what a
  wallet signs there.
  `personal_sign` / `eth_sign` / `signMessage` apply the envelope themselves, so handing them
  `digest` wraps it twice and recovers an address that is not the rater -- a well-formed signature
  that fails with `relay_bad_signature` and no other hint. `keccak256("\\x19Ethereum Signed
  Message:\\n32" || signingPayload) == digest`, so a client can check the two against each other
  instead of rebuilding the preimage from `data`
- `deadline`, `nonce` - the authorisation window and its single-use value
- `delegated` - whether the account already carries the delegation; when `false` the submission
  must include an EIP-7702 `authorization` signed by the rater, and `accountNonce` is the nonce
  to put in it

**Availability:** only where a `FeedbackDelegate` has actually been deployed and verified on-chain.
Today that is `base`, `ethereum`, `polygon`, `arbitrum`, `optimism`, `celo`, `bsc`, `monad` and
`base-sepolia`; other networks answer 400. The delegate takes its registry address through an
immutable constructor argument, so its address differs per chain and each one is verified
(`eth_getCode`, a `REPUTATION_REGISTRY()` read back, and an ERC-165 probe) before it is served.

**Two protocol versions are served in parallel, chosen per chain per request.** The ERC-165 probe
reports v4 (`0x378a0c90` → EIP-712 `typedData`) or v3 (`0x150b7a02` → the EIP-191 `digest`), and a
delegate that answers neither is a superseded v1 and is refused. Nothing about this is pinned to a
release: a chain starts serving `typedData` the moment a v4 delegate is deployed there, with no
deploy of ours in between.

`POST /feedback/response/evm/prepare` + `/feedback/response/evm/submit` are the same rail for
`appendResponse`, and they are **v4 only**: the v3 delegate accepts exactly two selectors and this
is not one of them, so a v3 network answers 400 `relay_response_needs_v4` instead of falling back
to the route where the FACILITATOR is the author on record. `clientAddress` and `feedbackIndex`
are inside the signed struct — without them one signature would answer any client's rating, or any
rating at that index.

A rater still pointed at a SUPERSEDED version of the delegate is reported as `delegated: false`,
not as an error — they sign a fresh authorisation and move to the current version. An account
delegated to somebody ELSE's implementation stays a 400: re-pointing it would break whatever
wallet provider put it there.

The ERC-165 probe is a **version** check, not a feature check. The delegates are deployed with
CREATE rather than CREATE2, so an address is a function of (deployer, nonce) and the same address
can hold a different version on a different chain. A superseded delegate still has code and is
still pinned to the right registry, so without this probe a stale entry would relay silently
against a version that breaks the rater's wallet. Such a delegate is refused with
`relay_delegate_superseded_version`.

`avalanche` is not on that list and is not waiting to join it: the C-Chain rejects the transaction
type itself (`-32000 transaction type not supported`), so relayed feedback is unavailable there by
design. Anchor the rating on a chain that supports EIP-7702 instead; the payment stays where it
was made.

Deadlines are deliberately short (default 15 minutes, `ERC8004_RELAY_DEADLINE_SECS`): relaying is
permissionless by design, so a signed authorisation is live in the wild until it expires.
"#,
    request_body(content = Object, description = "ERC-8004 feedback request with a `rater`"),
    responses(
        (status = 200, description = "Digest and parameters for the rater to sign", body = Object),
        (status = 400, description = "Missing rater, no delegate on this network, or the account is delegated elsewhere", body = Object),
        (status = 503, description = "Could not reach the chain, or the delegate is not usable", body = Object)
    )
)]
async fn path_feedback_evm_prepare() {}

#[utoipa::path(
    post,
    path = "/feedback/evm/submit",
    tag = "ERC-8004",
    summary = "Relay a rater-authored feedback (EVM, EIP-7702)",
    description = r#"
Relays a rating the rater authorised, as an EIP-7702 type-4 transaction sent to the rater's own
address. The facilitator pays the gas and never becomes the author.

Send back the same feedback parameters used for `/feedback/evm/prepare`, plus `deadline`, `nonce`,
the rater's `signature` over the prepared digest, and - when the account is not delegated yet - the
EIP-7702 `authorization`:

```json
{
  "x402Version": 1,
  "network": "base-sepolia",
  "feedback": { "...": "exactly what you sent to /prepare" },
  "deadline": 1786400000,
  "nonce": "0x2222...",
  "signature": "0x...",
  "authorization": {
    "chainId": 84532,
    "address": "0x3A68085499B62286468A35b7D9Dfc237ef2d3768",
    "nonce": 7,
    "yParity": 0,
    "r": "0x...",
    "s": "0x..."
  }
}
```

**The parameters are not redundant.** The facilitator rebuilds the registry calldata from them and
recomputes the digest; a signature that does not cover exactly that calldata is refused
(`relay_bad_signature`). It also refuses an authorization pointing at any delegate other than the
one it offered (`relay_authorization_wrong_delegate`) or signed by anyone other than the rater
(`relay_authorization_not_by_rater`) - otherwise a caller could have the facilitator pay to delegate
an account to a contract of their choosing.

The proof-of-payment gate applies here exactly as it does to `POST /feedback`: who authored a rating
and whether a payment backs it are separate questions.
"#,
    request_body(content = Object, description = "Feedback parameters, the rater's signature, and the EIP-7702 authorization"),
    responses(
        (status = 200, description = "Feedback relayed, authored by the rater", body = Object),
        (status = 400, description = "Signature, authorization, deadline or replay check failed", body = Object),
        (status = 500, description = "The relayed transaction failed", body = Object)
    )
)]
async fn path_feedback_evm_submit() {}

#[utoipa::path(
    post,
    path = "/feedback/solana/prepare",
    tag = "ERC-8004",
    summary = "Prepare a rater-signed feedback transaction (Solana)",
    description = r#"
Builds an UNSIGNED Solana transaction whose `client` account is the **rater**, for the rater to sign
in their own wallet. The facilitator remains the fee payer.

**Why this exists.** Account 0 of the program's `give_feedback` instruction is
`[signer, writable] client (feedback author / fee payer)`, and `POST /feedback` puts the
*facilitator's* keypair there - so the chain records the facilitator as the author of the rating,
not the person who made it. Solana supports several signers per transaction natively, so the rater
signs as `client` while the facilitator still pays. No delegation, no program change.

`rater` is REQUIRED here (base58 pubkey), and the returned transaction expects two signatures: the
fee payer's (added by `/feedback/solana/submit`) and the rater's.

```json
{
  "x402Version": 1,
  "network": "solana",
  "feedback": {
    "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
    "rater": "9oSLm8Rk1kQ9y8dFcqbAcTNqYqcrTUR6cQ4mL8mYNXpB",
    "value": 87, "valueDecimals": 0, "score": 95,
    "tag1": "quality", "tag2": "api"
  }
}
```

The response carries `transaction` (base64 of the bincode-serialised transaction), `blockhash` and
`lastValidBlockHeight`. Sign it and send it to `/feedback/solana/submit` before the blockhash expires.
"#,
    request_body(content = Object, description = "ERC-8004 feedback request with a `rater`"),
    responses(
        (status = 200, description = "Unsigned transaction for the rater to sign", body = Object),
        (status = 400, description = "Missing rater, unsupported network, or invalid parameters", body = Object),
        (status = 503, description = "Could not reach the network", body = Object)
    )
)]
async fn path_feedback_solana_prepare() {}

#[utoipa::path(
    post,
    path = "/feedback/solana/submit",
    tag = "ERC-8004",
    summary = "Submit a rater-signed feedback transaction (Solana)",
    description = r#"
Co-signs a rater-signed feedback transaction as fee payer and sends it.

Send back the SAME feedback parameters used for `/feedback/solana/prepare`, plus the transaction
with the rater's signature on it:

```json
{
  "x402Version": 1,
  "network": "solana",
  "feedback": { "...": "exactly what you sent to /prepare" },
  "transaction": "<base64 of the rater-signed transaction>"
}
```

**The parameters are not redundant.** The facilitator does not sign what it is given: it re-derives
the message from those parameters plus the blockhash carried by your submission, and refuses to
co-sign anything that is not byte-for-byte what it would have offered (`400`, error
`submitted transaction does not match the one this facilitator built`). Signing arbitrary blobs
would turn the fee-payer keypair into a public signing oracle - a single `system_program::transfer`
would empty the wallet with the facilitator's signature on it.

The rater's signature is verified *before* the facilitator adds its own, so a transaction the
network would reject never costs a fee.
"#,
    request_body(content = Object, description = "Feedback parameters plus the rater-signed transaction"),
    responses(
        (status = 200, description = "Feedback submitted, authored by the rater", body = Object),
        (status = 400, description = "Transaction does not match, or the rater's signature is missing or invalid", body = Object),
        (status = 500, description = "Submission failed", body = Object)
    )
)]
async fn path_feedback_solana_submit() {}

#[utoipa::path(
    post,
    path = "/feedback/revoke",
    tag = "ERC-8004",
    summary = "Revoke feedback (admin only)",
    description = r#"
Revokes previously submitted reputation feedback.

**Requires an `Authorization: Bearer <ERC8004_ADMIN_TOKEN>` header. When the server has no
ERC-8004 admin token configured this route answers 404, so it is indistinguishable from a
route that does not exist.**

The credential is deliberately NOT `BAZAAR_ADMIN_TOKEN`: the registry authorises
`revokeFeedback` by `msg.sender`, which is the facilitator, so this endpoint can erase any
feedback the registry attributes to the facilitator wallet - permanently, and for third
parties. It is gated separately from the catalog admin surface for that reason.

**EVM request:**
```json
{
  "x402Version": 1,
  "network": "base",
  "agentId": 42,
  "feedbackIndex": 1
}
```

**Solana request** needs the SEAL v1 hash of the feedback being revoked. Send the
content under `originalFeedback` and the facilitator derives it; the values must match
the original submission exactly.
```json
{
  "x402Version": 1,
  "network": "solana",
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "feedbackIndex": 1,
  "originalFeedback": {
    "value": 95, "valueDecimals": 0,
    "tag1": "uptime", "tag2": "verify",
    "endpoint": "https://api.example.com",
    "feedbackUri": "https://example.com/feedback.json"
  }
}
```
`sealHash: "0x..."` is still accepted if you computed it yourself (keccak256 over the
program's SEAL v1 layout) and takes precedence over `originalFeedback`.
"#,
    request_body(content = Object, description = "Revoke feedback request"),
    params(
        ("Authorization" = String, Header, description = "Bearer <ERC8004_ADMIN_TOKEN>")
    ),
    responses(
        (status = 200, description = "Revocation result", body = Object),
        (status = 400, description = "Revocation failed", body = Object),
        (status = 401, description = "Missing or invalid bearer token", body = Object),
        (status = 404, description = "Revoke surface disabled (no ERC8004_ADMIN_TOKEN configured)", body = Object)
    )
)]
async fn path_feedback_revoke() {}

#[utoipa::path(
    post,
    path = "/feedback/response",
    tag = "ERC-8004",
    summary = "Append response to feedback",
    description = r#"
Appends a response to existing feedback.

**This is NOT restricted to the agent, and the responder recorded on-chain is the facilitator.**
Verified against Base mainnet on 2026-08-18 by simulating `appendResponse` on a real feedback
entry: the call succeeds from an unrelated address, from the agent owner and from the facilitator
alike (with a negative control — the same call on a non-existent index reverts `index out of
bounds` — so the probe does distinguish success from failure).

Two consequences worth stating plainly rather than discovering later:

- the **registry** applies no access control, so anyone can attach a response to anyone's feedback;
- this **endpoint** is unauthenticated and the facilitator signs the transaction, so the
  `ResponseAppended` event records the FACILITATOR as `responder`. Real authorship here would need
  `appendResponse` added to the delegate's selector allowlist, which is a contract change.

Do not read a response as coming from the agent.

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

**EVM networks:** ethereum, base, polygon, arbitrum, optimism, celo, bsc, monad, avalanche, scroll + testnets

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
    "qualityScore": 78, "loyaltyScore": 64, "confidence": 85, "riskScore": 12,
    "diversityRatio": 67, "minScore": 40, "maxScore": 99, "lastScore": 95,
    "feedbackCount": 47, "lastFeedbackSlot": 301118422
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

**EVM networks:** ethereum, base, polygon, arbitrum, optimism, celo, bsc, monad, avalanche, scroll + testnets

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
    path = "/identity/{network}/owner/{address}",
    tag = "ERC-8004",
    summary = "Resolve an agent by its owner",
    description = r#"
Returns the first agent held by an address, for callers that need to know whether
an owner already has one before minting another.

**EVM networks:** `balanceOf` then batched `ownerOf` via Multicall3.

**Solana networks:** a `getProgramAccounts` scan filtered by the AgentAccount
discriminator and the `owner` field. `balance` is the number of agents matched.
The value read is `AgentAccount.owner`, which the registry caches from the Core
asset: an asset moved outside the registry's own transfer leaves it stale until
someone calls `sync_owner`.

```json
{
  "agentId": "247Y4QLwz9ZbcuHR2nX2EQLZHCsMs1GTqvgd6fpdn85Q",
  "owner": "6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv",
  "agentUri": "https://example.com/agent.json",
  "network": "solana",
  "balance": "1"
}
```

**404 means the owner holds nothing; 503 means the lookup could not reach a
verdict.** Treat them differently: persisting "not registered" from a 503 is how
a transient RPC failure becomes a permanent wrong answer, and on a mint path it
leads to minting a duplicate agent. A 503 carries `"retryable": true`.
"#,
    params(
        ("network" = String, Path, description = "Network name (e.g., base, solana, solana-devnet)"),
        ("address" = String, Path, description = "Owner address: 0x-hex for EVM, base58 for Solana")
    ),
    responses(
        (status = 200, description = "Agent found", body = Object),
        (status = 400, description = "Invalid network or address", body = Object),
        (status = 404, description = "Address owns no agent on this network", body = Object),
        (status = 503, description = "Lookup inconclusive, retry", body = Object)
    )
)]
async fn path_identity_by_owner() {}

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
The ATOM Engine tracks quality through EMA scores, not positive/negative tallies, so
there are no such counters.

**Solana networks** read the Metaplex Core collection referenced by the RootConfig PDA:
`totalSupply` is its `current_size` (net of burns) and `numMinted` its all-time mint count.
The registry itself keeps no agent counter on-chain.

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

// ============================================================================
// Agentic Discovery Surfaces
//
// The static documents an agent or a scanner fetches BEFORE it knows how to
// call anything. Served by `handlers::agentic_routes()`; documented here because
// a route that is not in this file is invisible in /docs and to every client
// generated from the spec.
// ============================================================================

#[utoipa::path(
    get,
    path = "/llms.txt",
    tag = "Agentic",
    summary = "llms.txt site map for LLMs",
    description = "The llmstxt.org map of this service: what it is, what it costs (nothing), which networks and schemes it settles, and where every other machine-readable document lives.",
    responses(
        (status = 200, description = "Plain-text map", content_type = "text/plain", body = String)
    )
)]
async fn path_llms_txt() {}

#[utoipa::path(
    get,
    path = "/llms-full.txt",
    tag = "Agentic",
    summary = "llms.txt, index.md, skill.md and auth.md in one file",
    description = "The whole agent-facing documentation set concatenated, for pasting into one context window. Generated by `scripts/build_llms_full.sh`; a test fails the build when it drifts from its sources.",
    responses(
        (status = 200, description = "Plain-text bundle", content_type = "text/plain", body = String)
    )
)]
async fn path_llms_full_txt() {}

#[utoipa::path(
    get,
    path = "/robots.txt",
    tag = "Agentic",
    summary = "Crawler policy",
    description = "RFC 9309 policy with every AI crawler allowed explicitly, Content-Signal set to yes on all three signals, and no Disallow: this service is public payment infrastructure with no paid routes to hide.",
    responses(
        (status = 200, description = "Crawler policy", content_type = "text/plain", body = String)
    )
)]
async fn path_robots_txt() {}

#[utoipa::path(
    get,
    path = "/sitemap.xml",
    tag = "Agentic",
    summary = "Sitemap",
    description = "The pages a reader can land on: the HTML pages and the agent-facing Markdown documents.",
    responses(
        (status = 200, description = "Sitemap", content_type = "application/xml", body = String)
    )
)]
async fn path_sitemap_xml() {}

#[utoipa::path(
    get,
    path = "/index.md",
    tag = "Agentic",
    summary = "Landing page in Markdown",
    description = "The landing page as text, for an agent that would rather not render a 240 KB HTML monolith to learn what this service is.",
    responses(
        (status = 200, description = "Markdown overview", content_type = "text/markdown", body = String)
    )
)]
async fn path_index_md() {}

#[utoipa::path(
    get,
    path = "/skill.md",
    tag = "Agentic",
    summary = "Agent operating manual",
    description = "How to call verify and settle: the request and response shapes, the five schemes, the per-chain EIP-712 domain-name trap, and which failures mean retry rather than stop.",
    responses(
        (status = 200, description = "Markdown manual", content_type = "text/markdown", body = String)
    )
)]
async fn path_skill_md() {}

#[utoipa::path(
    get,
    path = "/auth.md",
    tag = "Agentic",
    summary = "Authentication guide",
    description = "How to authenticate against this facilitator, which is: you do not. No accounts, no API keys, no OAuth. Documents the per-IP rate limits and the operator-only admin routes instead.",
    responses(
        (status = 200, description = "Markdown auth guide", content_type = "text/markdown", body = String)
    )
)]
async fn path_auth_md() {}

#[utoipa::path(
    get,
    path = "/workflows.json",
    tag = "Agentic",
    summary = "Workflow manifest",
    description = "The four state machines this facilitator drives (one-shot payment, two-phase escrow, asynchronous ERC-8004 registration, feedback prepare/submit), with the operation that triggers each transition.",
    responses(
        (status = 200, description = "Workflow manifest", body = Object)
    )
)]
async fn path_workflows_json() {}

#[utoipa::path(
    get,
    path = "/openapi.json",
    tag = "Agentic",
    summary = "This document",
    description = "The OpenAPI specification, at the root path scanners and RFC 9727 catalogs look for. Identical to `/api-docs/openapi.json`, which is where Swagger UI reads it.",
    responses(
        (status = 200, description = "OpenAPI 3.1 document", body = Object),
        (status = 500, description = "The document could not be serialised", body = Object)
    )
)]
async fn path_openapi_json() {}

#[utoipa::path(
    get,
    path = "/.well-known/agent-card.json",
    tag = "Agentic",
    summary = "A2A agent card",
    description = "The agent-to-agent card: identity, transport, capabilities and skills (verify, settle, supported, accepts).",
    responses(
        (status = 200, description = "A2A agent card", body = Object)
    )
)]
async fn path_agent_card() {}

#[utoipa::path(
    get,
    path = "/.well-known/agent.json",
    tag = "Agentic",
    summary = "A2A agent card (legacy path)",
    description = "Byte-identical to `/.well-known/agent-card.json`. Several clients still look here first, and two cards that could disagree is worse than one served twice.",
    responses(
        (status = 200, description = "A2A agent card", body = Object)
    )
)]
async fn path_agent_json_legacy() {}

#[utoipa::path(
    get,
    path = "/.well-known/x402",
    tag = "Agentic",
    summary = "x402 discovery document",
    description = "Declares `role: \"facilitator\"`, the verify/settle/supported endpoints, the five schemes and the networks. `paidRoutes` is empty because this service charges nothing and none of its routes answer 402. Token contract addresses are deliberately absent so the document cannot drift from the code; read them from `POST /accepts`.",
    responses(
        (status = 200, description = "x402 discovery document", body = Object)
    )
)]
async fn path_x402_discovery() {}

#[utoipa::path(
    get,
    path = "/.well-known/api-catalog",
    tag = "Agentic",
    summary = "API catalog (RFC 9727)",
    description = "The linkset: service-desc to the OpenAPI document, service-doc to the Swagger UI, service-meta to the agent manual, status to the health endpoint.",
    responses(
        (status = 200, description = "RFC 9727 linkset", content_type = "application/linkset+json", body = Object)
    )
)]
async fn path_api_catalog() {}

#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource",
    tag = "Agentic",
    summary = "Protected resource metadata (RFC 9728)",
    description = "Published with an empty `authorization_servers` on purpose: it exists so an OAuth-capable agent can discover in one GET that this resource is NOT OAuth-protected, instead of trying and failing.",
    responses(
        (status = 200, description = "RFC 9728 metadata", body = Object)
    )
)]
async fn path_oauth_protected_resource() {}

#[utoipa::path(
    get,
    path = "/.well-known/agent-skills/index.json",
    tag = "Agentic",
    summary = "Agent skills index",
    description = "One entry, pointing at `/skill.md` with its real sha256 digest. A test fails the build if the digest stops matching the file.",
    responses(
        (status = 200, description = "Skills index", body = Object)
    )
)]
async fn path_agent_skills_index() {}

#[utoipa::path(
    get,
    path = "/.well-known/mcp/server-card.json",
    tag = "Agentic",
    summary = "MCP server card",
    description = "Where the MCP server lives and what it can do: `transport.endpoint` is `POST /mcp`, and the `tools` array is the same four names `tools/list` returns. `serverInfo.version` is stamped at runtime from the running release, so it cannot go stale.",
    responses(
        (status = 200, description = "MCP server card", body = Object)
    )
)]
async fn path_mcp_server_card() {}

#[utoipa::path(
    post,
    path = "/mcp",
    tag = "MCP",
    summary = "MCP endpoint (JSON-RPC 2.0 over Streamable HTTP)",
    description = r#"
The facilitator as an MCP server. Stateless Streamable HTTP: every request is a
JSON-RPC 2.0 document, there is no session id, and `GET /mcp` answers 405 because
there is no server-initiated stream to open.

**Tools** (each one is dispatched through the REST handler it names, so an MCP call
and the HTTP call it stands for cannot answer differently):

| Tool | Is | Moves money |
|---|---|---|
| `x402_supported` | `GET /supported` | no |
| `x402_accepts` | `POST /accepts` | no |
| `x402_verify` | `POST /verify` | no |
| `x402_settle` | `POST /settle` | yes, irreversibly |

A tool's `arguments` are the JSON body of the request it stands for; its result is
that request's response body verbatim in one text content block. A non-2xx answer
comes back as `isError: true` carrying the facilitator's own message, not as a
JSON-RPC error.

The body is the only channel: a tool call cannot set headers. The one exception is
`x402_settle`, which takes an optional `idempotencyKey` argument that is lifted out
of the body and sent as the `Idempotency-Key` header, so an MCP client can ask for
exactly-once the same way an HTTP client does. The v2 `PAYMENT-SIGNATURE` header
transport has no equivalent here; send the payload in the body.

**`Accept` must name BOTH `application/json` and `text/event-stream`.** The MCP
Streamable HTTP transport requires it and answers `406` otherwise, even though this
server is stateless and always replies with JSON. Use
`accept: application/json, text/event-stream`.

**Authentication:** none, same as every other route (`/auth.md`).
**Rate limit:** shared with `POST /verify` and `POST /settle` -- one per-IP bucket,
not two.

Handshake (note the two Accept types):

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocolVersion":"2025-06-18","capabilities":{},
  "clientInfo":{"name":"my-agent","version":"1.0"}}}
```

Tool call:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
  "name":"x402_supported","arguments":{}}}
```
"#,
    request_body(content = Object, description = "A JSON-RPC 2.0 request: initialize, tools/list, tools/call, ping"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response", body = Object,
            example = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "x402-facilitator", "version": "0.0.0" }
                }
            })
        ),
        (status = 400, description = "Not a JSON-RPC document", body = Object),
        (status = 403, description = "Host header not on the MCP allowlist (MCP_ALLOWED_HOSTS)", body = Object),
        (status = 406, description = "Accept did not name both application/json and text/event-stream", body = Object,
            example = json!({
                "error": "Not Acceptable",
                "status": 406,
                "hint": "Accept must name BOTH application/json and text/event-stream, e.g. `accept: application/json, text/event-stream`."
            })
        ),
        (status = 415, description = "Content-Type is not application/json", body = Object),
        (status = 429, description = "Per-IP rate limit, shared with /verify and /settle", body = Object)
    )
)]
async fn path_mcp_post() {}

#[utoipa::path(
    get,
    path = "/mcp",
    tag = "MCP",
    summary = "No SSE stream here",
    description = "Always 405 with `Allow: POST`. This MCP server is stateless, so there is no server-initiated event stream to subscribe to. The body is JSON, not text, so a scanner grading content types does not read it as a broken surface.",
    responses(
        (status = 405, description = "Use POST", body = Object,
            example = json!({
                "error": "GET is not supported on /mcp",
                "transport": "streamable-http",
                "method": "POST"
            })
        )
    )
)]
async fn path_mcp_get() {}

/// Create the Swagger UI router.
///
/// The OpenAPI version is patched at runtime from the `FACILITATOR_VERSION` env (see `src/version.rs`),
/// so it always stays in sync without manual updates.
pub fn swagger_routes() -> Router {
    let mut api_doc = ApiDoc::openapi();
    api_doc.info.version = crate::version::facilitator_version().to_string();

    // `/openapi.json` is an ALIAS for the document Swagger UI already serves at
    // `/api-docs/openapi.json`. Two reasons it exists, and neither is cosmetic:
    //
    //   1. Every agentic scanner and every RFC 9727 catalog looks for the spec
    //      at the well-known root path. Served only under `/api-docs/`, a
    //      complete OpenAPI document is invisible to all of them.
    //   2. `.well-known/api-catalog` declares `service-desc` -> `/openapi.json`.
    //      A catalog that points at a 404 is worse than no catalog.
    //
    // Serialised ONCE here rather than per request: the document is large and
    // does not change while the process runs. `Bytes` so the per-request clone is
    // a refcount bump and not a copy of the whole spec. If serialisation ever
    // failed the route answers 500 rather than a truncated body -- a half-spec is
    // harder to debug than an honest error.
    let spec_json: Option<Bytes> = match serde_json::to_string(&api_doc) {
        Ok(body) => Some(Bytes::from(body)),
        Err(e) => {
            tracing::error!(error = %e, "failed to serialise the OpenAPI document");
            None
        }
    };

    Router::new()
        .route(
            "/openapi.json",
            get(move || {
                let spec = spec_json.clone();
                async move {
                    match spec {
                        Some(body) => (
                            StatusCode::OK,
                            [("content-type", "application/json; charset=utf-8")],
                            body,
                        )
                            .into_response(),
                        None => (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            [("content-type", "application/json; charset=utf-8")],
                            Bytes::from_static(b"{\"error\":\"openapi document unavailable\"}"),
                        )
                            .into_response(),
                    }
                }
            }),
        )
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", api_doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every DX402 route the router serves must appear in the spec.
    ///
    /// CLAUDE.md says every new endpoint needs `src/openapi.rs` edited, and a
    /// rule that only lives in prose gets skipped. A `utoipa::path` that is
    /// written but never added to the `paths(...)` list compiles, runs, and is
    /// simply absent from `/api-docs/openapi.json` -- there is nothing to
    /// notice, which is why this is a test and not a convention.
    ///
    /// Listed literally rather than derived from `dx402_routes()`: axum's
    /// `Router` does not expose its own paths, so the honest options are a
    /// hand-kept list that fails loudly or no check at all.
    #[test]
    fn every_dx402_route_is_documented() {
        let spec = ApiDoc::openapi();
        for route in [
            "/dx402/anchor",
            "/dx402/evidence/{paymentId}",
            "/dx402/receipt/{paymentId}",
            "/dx402/blob/{paymentId}",
            "/dx402/stats",
            "/dx402/recover",
            "/dx402/repair/{paymentId}",
        ] {
            assert!(
                spec.paths.paths.contains_key(route),
                "{route} is served but missing from the OpenAPI spec, so it is \
                 invisible in /docs and to every client generated from it"
            );
        }
    }

    /// The version in the spec is the release, resolved at runtime.
    ///
    /// The `#[openapi(version = ...)]` attribute is a placeholder that
    /// `swagger_routes` overwrites; asserting they differ would pin the
    /// placeholder, so assert the override instead.
    #[test]
    fn the_spec_reports_the_running_release() {
        let mut spec = ApiDoc::openapi();
        spec.info.version = crate::version::facilitator_version().to_string();
        assert_eq!(spec.info.version, crate::version::facilitator_version());
        assert!(!spec.info.version.is_empty());
    }

    /// `/openapi.json` serves the spec, as JSON, at the path scanners look at.
    ///
    /// The document already existed at `/api-docs/openapi.json`, which is where
    /// Swagger UI wants it and nowhere an agentic scanner or an RFC 9727 catalog
    /// looks. `.well-known/api-catalog` declares `service-desc` -> `/openapi.json`,
    /// so this route is what keeps that declaration from pointing at a 404.
    #[tokio::test]
    async fn the_openapi_document_is_also_served_at_the_root_path() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let response = swagger_routes()
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let ctype = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ctype.starts_with("application/json"),
            "/openapi.json answered content-type {ctype:?}"
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("must be valid JSON");
        assert!(doc.get("openapi").is_some(), "missing the `openapi` field");
        assert!(doc.get("paths").is_some(), "missing the `paths` field");
        assert_eq!(
            doc["info"]["version"],
            crate::version::facilitator_version()
        );
    }

    /// Every agentic-discovery surface the router serves is in the spec.
    ///
    /// Same reasoning as `every_dx402_route_is_documented`: a `utoipa::path`
    /// that is written but never added to `paths(...)` compiles, runs, and is
    /// simply absent from the document. Listed literally because axum's `Router`
    /// will not enumerate its own paths.
    #[test]
    fn every_agentic_surface_is_documented() {
        let spec = ApiDoc::openapi();
        for route in [
            "/llms.txt",
            "/llms-full.txt",
            "/robots.txt",
            "/sitemap.xml",
            "/index.md",
            "/skill.md",
            "/auth.md",
            "/workflows.json",
            "/openapi.json",
            "/.well-known/agent-card.json",
            "/.well-known/agent.json",
            "/.well-known/x402",
            "/.well-known/api-catalog",
            "/.well-known/oauth-protected-resource",
            "/.well-known/agent-skills/index.json",
        ] {
            assert!(
                spec.paths.paths.contains_key(route),
                "{route} is served but missing from the OpenAPI spec, so it is \
                 invisible in /docs and to every client generated from it"
            );
        }
    }
}
