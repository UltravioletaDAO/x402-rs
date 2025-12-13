# Zama FHE x402 Integration

**Status:** Completed
**Version:** v1.9.0
**Date:** 2025-12-13

## Overview

This document describes the integration of Zama's Fully Homomorphic Encryption (FHE) payment scheme into the x402-rs payment facilitator. The integration enables confidential token transfers using ERC7984 tokens on Zama's FHEVM, while maintaining full backward compatibility with existing `exact` scheme payments.

## Architecture

### High-Level Flow

```
                                 x402 Payment Request
                                         |
                                         v
                    +--------------------------------------------+
                    |     facilitator.ultravioletadao.xyz        |
                    |              (x402-rs v1.9.0)              |
                    |                                            |
                    |   +------------------------------------+   |
                    |   |        Scheme Router               |   |
                    |   |   (handlers.rs verify/settle)      |   |
                    |   +------------------------------------+   |
                    |          |                    |            |
                    |     scheme="exact"      scheme="fhe-transfer"
                    |          |                    |            |
                    +----------|--------------------|-----------+
                               |                    |
                               v                    v
                    +------------------+   +------------------------+
                    |  Local Processing |   |     FHE Proxy          |
                    |                    |   |   (fhe_proxy.rs)       |
                    |  - EVM chains      |   |                        |
                    |  - Solana          |   |   HTTP Client to       |
                    |  - NEAR            |   |   Lambda endpoint      |
                    |  - Stellar         |   |                        |
                    +------------------+   +------------------------+
                                                     |
                                                     v
                               +------------------------------------------+
                               |   zama-facilitator.ultravioletadao.xyz   |
                               |          (AWS Lambda + API Gateway)       |
                               |                                           |
                               |   - TFHE WASM runtime                     |
                               |   - KMS integration                       |
                               |   - ERC7984 token verification            |
                               |   - Zama FHEVM settlement                 |
                               +------------------------------------------+
                                                     |
                                                     v
                               +------------------------------------------+
                               |          Ethereum Sepolia (testnet)       |
                               |                                           |
                               |   - Zama FHEVM contracts                  |
                               |   - ERC7984 confidential tokens           |
                               |   - Encrypted balances & transfers        |
                               +------------------------------------------+
```

### Component Interaction Sequence

```
Client                Main Facilitator              FHE Proxy              Zama Lambda
  |                         |                          |                        |
  |  POST /verify           |                          |                        |
  |  scheme: fhe-transfer   |                          |                        |
  |------------------------>|                          |                        |
  |                         |                          |                        |
  |                         |  Check scheme            |                        |
  |                         |  == "fhe-transfer"       |                        |
  |                         |------------------------->|                        |
  |                         |                          |                        |
  |                         |                          |  POST /verify          |
  |                         |                          |----------------------->|
  |                         |                          |                        |
  |                         |                          |                        | FHE Verification:
  |                         |                          |                        | - Decrypt amount
  |                         |                          |                        | - Verify signature
  |                         |                          |                        | - Check balance
  |                         |                          |                        |
  |                         |                          |  { isValid: true }     |
  |                         |                          |<-----------------------|
  |                         |                          |                        |
  |                         |  FheVerifyResponse       |                        |
  |                         |<-------------------------|                        |
  |                         |                          |                        |
  |  { isValid: true }      |                          |                        |
  |<------------------------|                          |                        |
  |                         |                          |                        |
```

## Implementation Details

### New Files

| File | Description |
|------|-------------|
| `src/fhe_proxy.rs` | HTTP client module for forwarding FHE requests to Lambda |
| `terraform/environments/zama-testnet/` | Terraform infrastructure for Lambda + API Gateway |
| `docs/ZAMA_FHE_INTEGRATION.md` | This documentation |

### Modified Files

| File | Changes |
|------|---------|
| `src/types.rs` | Added `Scheme::FheTransfer` variant |
| `src/lib.rs` | Added `pub mod fhe_proxy;` |
| `src/main.rs` | Added `mod fhe_proxy;` |
| `src/handlers.rs` | Added FHE routing in `verify` and `settle` handlers |
| `src/facilitator_local.rs` | Added `fhe-transfer` to `/supported` endpoint |
| `Cargo.toml` | Version bump 1.8.0 -> 1.9.0 |

### Code Structure

```
src/
├── fhe_proxy.rs          # NEW: FHE proxy module
│   ├── FheProxyConfig    # Configuration (endpoint URL, timeout)
│   ├── FheProxy          # HTTP client for Lambda
│   ├── FheVerifyResponse # Response type from Lambda
│   └── FheProxyError     # Error types
│
├── types.rs              # MODIFIED
│   └── Scheme            # Added FheTransfer variant
│       ├── Exact         # Standard EIP-3009 transfers
│       └── FheTransfer   # NEW: Zama FHEVM transfers
│
├── handlers.rs           # MODIFIED
│   ├── verify()          # Added FHE routing check
│   └── settle()          # Added FHE routing check
│
└── facilitator_local.rs  # MODIFIED
    └── supported_kinds() # Added fhe-transfer entries
```

### FHE Proxy Module (`src/fhe_proxy.rs`)

```rust
/// Configuration for the FHE proxy
pub struct FheProxyConfig {
    /// Base URL of the Zama FHE facilitator Lambda
    /// Default: https://zama-facilitator.ultravioletadao.xyz
    pub endpoint: String,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

/// FHE Proxy client for forwarding requests to Zama Lambda
pub struct FheProxy {
    client: Client,
    config: FheProxyConfig,
}

impl FheProxy {
    pub async fn verify(&self, body: &serde_json::Value) -> Result<FheVerifyResponse, FheProxyError>;
    pub async fn settle(&self, body: &serde_json::Value) -> Result<serde_json::Value, FheProxyError>;
    pub async fn health_check(&self) -> Result<bool, FheProxyError>;
}
```

### Scheme Routing Logic (`src/handlers.rs`)

```rust
// In verify() handler:
if v1_request.payment_payload.scheme == Scheme::FheTransfer {
    info!("Routing fhe-transfer request to Zama Lambda facilitator");

    match FHE_PROXY.verify(&json_body).await {
        Ok(fhe_response) => {
            return (StatusCode::OK, Json(fhe_response)).into_response();
        }
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, Json(error_response)).into_response();
        }
    }
}

// Standard exact scheme - process locally
match facilitator.verify(&v1_request).await { ... }
```

## Infrastructure

### AWS Architecture

```
                         Route 53
                            |
            +---------------+---------------+
            |                               |
            v                               v
    facilitator.                    zama-facilitator.
    ultravioletadao.xyz             ultravioletadao.xyz
            |                               |
            v                               v
    +---------------+               +---------------+
    |     ALB       |               | API Gateway   |
    | (us-east-2)   |               | HTTP API      |
    +---------------+               +---------------+
            |                               |
            v                               v
    +---------------+               +---------------+
    |  ECS Fargate  |               | Lambda        |
    |  x402-rs      |  -----------> | x402-zama     |
    |  v1.9.0       |  (HTTP proxy) | v1.0.0        |
    +---------------+               +---------------+
            |                               |
            v                               v
    Multiple chains:                Ethereum Sepolia:
    - Base, Polygon                 - Zama FHEVM
    - Optimism, Celo                - ERC7984 tokens
    - Solana, NEAR
    - Stellar, etc.
```

### Terraform Resources (`terraform/environments/zama-testnet/`)

```hcl
# Lambda Function
resource "aws_lambda_function" "zama_facilitator" {
  function_name = "zama-facilitator"
  runtime       = "nodejs20.x"
  handler       = "handler.handler"
  memory_size   = 512
  timeout       = 30
}

# API Gateway HTTP API
resource "aws_apigatewayv2_api" "zama_api" {
  name          = "zama-facilitator-api"
  protocol_type = "HTTP"
}

# Custom Domain
resource "aws_apigatewayv2_domain_name" "zama_domain" {
  domain_name = "zama-facilitator.ultravioletadao.xyz"
}

# CloudWatch Alarms
resource "aws_cloudwatch_metric_alarm" "lambda_errors" {
  alarm_name  = "zama-facilitator-errors"
  metric_name = "Errors"
  threshold   = 5
}
```

## API Reference

### Supported Payment Kinds

The `/supported` endpoint now returns FHE transfer schemes:

```json
{
  "kinds": [
    // ... existing exact schemes ...

    // FHE Transfer - v1 format
    {
      "network": "ethereum-sepolia",
      "scheme": "fhe-transfer",
      "x402Version": 1
    },

    // FHE Transfer - v2 CAIP-2 format
    {
      "network": "eip155:11155111",
      "scheme": "fhe-transfer",
      "x402Version": 2
    }
  ]
}
```

### Verify Request (FHE)

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/verify \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "paymentPayload": {
      "scheme": "fhe-transfer",
      "network": "ethereum-sepolia",
      "payload": {
        "encryptedAmount": "0x...",
        "signature": "0x...",
        "from": "0x...",
        "to": "0x...",
        "token": "0x..."
      }
    },
    "paymentRequirements": {
      "scheme": "fhe-transfer",
      "network": "ethereum-sepolia",
      "maxAmountRequired": "1000000",
      "resource": "https://api.example.com/protected"
    }
  }'
```

### Verify Response (FHE)

```json
{
  "isValid": true,
  "payer": "0x1234567890abcdef...",
  "decryptedAmount": "1000000"
}
```

### Settle Request (FHE)

```bash
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "paymentPayload": {
      "scheme": "fhe-transfer",
      "network": "ethereum-sepolia",
      "payload": { ... }
    },
    "paymentRequirements": { ... }
  }'
```

## Environment Variables

### Main Facilitator (x402-rs)

| Variable | Default | Description |
|----------|---------|-------------|
| `FHE_FACILITATOR_URL` | `https://zama-facilitator.ultravioletadao.xyz` | Zama Lambda endpoint |

### Zama Lambda

| Variable | Description |
|----------|-------------|
| `GATEWAY_URL` | Zama KMS gateway for decryption |
| `ACL_CONTRACT_ADDRESS` | Access control contract |
| `KMS_CONTRACT_ADDRESS` | KMS verifier contract |
| `PRIVATE_KEY` | Lambda wallet for settlements |

## Testing

### Health Checks

```bash
# Main facilitator
curl https://facilitator.ultravioletadao.xyz/version
# {"version":"1.9.0"}

# Zama Lambda
curl https://zama-facilitator.ultravioletadao.xyz/health
# {"status":"ok","service":"x402-facilitator","version":"1.0.0",...}
```

### Verify FHE Scheme Available

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | \
  python3 -c "import sys,json; d=json.load(sys.stdin); \
  print([k for k in d['kinds'] if k['scheme']=='fhe-transfer'])"
```

## Deployment

### Build and Deploy

```bash
# Build Docker image
./scripts/build-and-push.sh v1.9.0

# Update ECS task definition and deploy
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:71 \
  --force-new-deployment \
  --region us-east-2
```

### Verify Deployment

```bash
# Check version
curl https://facilitator.ultravioletadao.xyz/version

# Check FHE scheme
curl -s https://facilitator.ultravioletadao.xyz/supported | grep fhe-transfer
```

## Security Considerations

1. **Lambda Isolation**: FHE operations run in isolated Lambda environment
2. **KMS Integration**: Decryption keys managed by Zama KMS
3. **Network Separation**: Lambda has minimal IAM permissions
4. **Request Validation**: All requests validated before proxying
5. **Error Handling**: FHE errors don't expose internal details

## Future Enhancements

1. **Mainnet Support**: Add Ethereum mainnet when Zama FHEVM launches
2. **Additional Networks**: Support other FHEVM-compatible chains
3. **Caching**: Cache Lambda responses for repeated verifications
4. **Metrics**: Add detailed FHE-specific metrics and dashboards
5. **Circuit Breaker**: Implement circuit breaker for Lambda failures

## References

- [Zama FHEVM Documentation](https://docs.zama.ai/fhevm)
- [ERC7984 Standard](https://eips.ethereum.org/EIPS/eip-7984)
- [x402 Protocol Specification](https://x402.org)
- [x402-rs Repository](https://github.com/UltravioletaDAO/x402-rs)
