# x402-zama Integration Plan

## Executive Summary

This document outlines the plan to integrate Zama FHE (Fully Homomorphic Encryption) payments into the Ultravioleta DAO x402 payment ecosystem. The integration enables privacy-preserving payments where transaction amounts remain encrypted on-chain.

**Repository**: https://github.com/tomi204/x402-zama
**Documentation**: https://x402-fhe-docs.vercel.app/

---

## Approval Status

| Item | Status | Date |
|------|--------|------|
| **Plan Approved** | YES | 2025-12-12 |
| **Budget Approved** | YES | 2025-12-12 |
| **Zama Testnet Validated** | YES | 2025-12-12 |
| **Phase 1 (Infrastructure)** | COMPLETE | 2025-12-12 |
| **Phase 4 (Rust Integration)** | **ACTIVE - Option B** | Changed 2025-12-12 |

### Key Decisions

1. **x402 v1/v2 Compatibility**: x402-zama supports v1 protocol. Will verify during implementation.
2. **Hono-Lambda Refactoring**: Approved - will refactor to use `hono/aws-lambda` adapter.
3. **IAM Permissions**: Added to Terraform template (Secrets Manager access).
4. **Package Size**: Will attempt standard deployment; Docker fallback if >50MB.
5. **CORS Handling**: Will implement proper OPTIONS preflight handling.
6. **Architecture: Option B (Proxy)**: Single endpoint `facilitator.ultravioletadao.xyz` proxies `fhe-transfer` to Lambda.

### Architecture Decision: Option B (Proxy Integration)

**Single Endpoint**: `https://facilitator.ultravioletadao.xyz`

- All payment schemes (EIP-3009, Solana, NEAR, Stellar, **FHE**) go through x402-rs
- x402-rs detects `fhe-transfer` scheme and proxies to Lambda internally
- Lambda does NOT need custom domain (internal use only via API Gateway default URL)
- Clients don't need to know about multiple facilitators

### Effort Estimate (Approved - Full Scope)

| Phase | Hours |
|-------|-------|
| Phase 1 (Infrastructure) | 6-7 | **COMPLETE** |
| Phase 2 (Code Deployment) | 7-9 |
| Phase 3 (Testing) | 8-10 |
| Phase 4 (Rust Integration) | 6-8 |
| **Total** | **27-34 hours** |
| Contingency (+50%) | 40-51 hours |

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [Phase 1: AWS Lambda Deployment](#3-phase-1-aws-lambda-deployment)
4. [Phase 2: Facilitator Integration](#4-phase-2-facilitator-integration)
5. [Phase 3: Testing & Validation](#5-phase-3-testing--validation)
6. [Technical Details](#6-technical-details)
7. [Cost Estimates](#7-cost-estimates)
8. [Risks & Mitigations](#8-risks--mitigations)
9. [Task Breakdown](#9-task-breakdown)

---

## 1. Overview

### What is x402-zama?

The x402-zama facilitator is a TypeScript service that verifies FHE (Fully Homomorphic Encryption) payments using Zama's FHEVM. Unlike standard EIP-3009 payments (which our x402-rs handles), FHE payments:

- **Encrypt amounts on-chain**: Transfer amounts are never visible publicly
- **Use ERC7984 tokens**: Confidential token standard with `ConfidentialTransfer` events
- **Require ZAMA's relayer**: Decryption happens through external FHE infrastructure
- **Use `fhe-transfer` scheme**: Different from our current `exact` (EIP-3009) scheme

### Why Integrate?

1. **Privacy-preserving payments**: Users can pay for resources without revealing amounts
2. **Expanding x402 ecosystem**: First FHE implementation in x402 protocol
3. **Testnet availability**: Zama's Sepolia coprocessor is ready for testing

### Current State

- **Zama Testnet**: Available on Ethereum Sepolia (Chain ID: 11155111)
- **Relayer**: https://relayer.testnet.zama.cloud
- **Gateway**: https://gateway.sepolia.zama.ai/
- **x402-zama repo**: Complete facilitator implementation in TypeScript/Hono

---

## 2. Architecture

### 2.1 Current x402-rs Architecture

```
                    +------------------+
                    |   x402-rs        |
                    |   Facilitator    |
Client  ------>     |   (Rust/Axum)    |  -----> Blockchain
                    |                  |
                    | /verify, /settle |
                    +------------------+
                          |
                          v
              +------------------------+
              | Supported Schemes:     |
              | - exact (EIP-3009)     |
              | - solana               |
              | - near                 |
              | - stellar              |
              +------------------------+
```

### 2.2 Proposed Architecture: Option B (Proxy Integration)

```
                                                        +------------------+
                                                        |  x402-zama       |
                                                        |  Lambda          |
                                                   +--->|  (TS/Hono)       |---> ZAMA Relayer
                                                   |    |                  |     (Decryption)
                                                   |    | /verify          |
+------------------+                               |    +------------------+
|   x402-rs        |                               |
|   Facilitator    |   fhe-transfer scheme         |
|   (Rust/Axum)    |-------------------------------+
|                  |
| /verify, /settle |-------> EVM (EIP-3009)
|                  |-------> Solana
|                  |-------> NEAR
|                  |-------> Stellar
+------------------+
        ^
        |
        |   ALL payment schemes through single endpoint
        |
+----------------------------------------------------------+
|                         Clients                          |
|        https://facilitator.ultravioletadao.xyz           |
+----------------------------------------------------------+

Internal Lambda URL: API Gateway default (no custom domain needed)
Example: https://abc123.execute-api.us-east-2.amazonaws.com
```

**Single Endpoint**: `https://facilitator.ultravioletadao.xyz` handles ALL schemes including FHE.

**Flow for FHE payments**:
1. Client sends `fhe-transfer` scheme request to x402-rs
2. x402-rs detects scheme, proxies to Lambda (internal URL)
3. Lambda verifies with ZAMA relayer
4. x402-rs returns response to client

### 2.3 Integration Options

| Option | Description | Pros | Cons |
|--------|-------------|------|------|
| **A: Standalone Lambda** | x402-zama runs independently, clients call directly | Simple, no x402-rs changes | Two endpoints to manage |
| **B: Proxy through x402-rs** | x402-rs routes `fhe-transfer` to Lambda | Single endpoint | Adds latency, coupling |
| **C: Embed in x402-rs** | Port TypeScript to Rust | Single codebase | Major rewrite, FHE SDK not in Rust |

**Recommendation**: **Option A (Standalone Lambda)** for initial deployment, with potential migration to Option B later.

---

## 3. Phase 1: AWS Lambda Deployment

### 3.1 Infrastructure Components

```
terraform/
  environments/
    zama-testnet/          # New environment
      main.tf              # Lambda + API Gateway
      variables.tf
      outputs.tf
      backend.tf
```

### 3.2 Terraform Resources Required

1. **Lambda Function**
   - Runtime: Node.js 20.x
   - Handler: Bundled Hono app
   - Memory: 1024 MB (for FHE operations)
   - Timeout: 30 seconds (FHE decryption can be slow)
   - Provisioned Concurrency: 1 (mitigate cold starts)

2. **API Gateway (HTTP API v2)**
   - Custom domain: `zama-facilitator.ultravioletadao.xyz`
   - Routes: `/verify`, `/health`
   - CORS enabled

3. **CloudWatch Logs**
   - Log group: `/aws/lambda/zama-facilitator`
   - Retention: 14 days

4. **IAM Roles**
   - Lambda execution role
   - CloudWatch Logs access

5. **ACM Certificate**
   - For custom domain SSL

6. **Route53 Record**
   - A record pointing to API Gateway

### 3.3 Lambda Bundle Preparation

The x402-zama facilitator needs to be bundled for Lambda:

```bash
# In /tmp/x402-zama/packages/x402-facilitator
pnpm install
pnpm build

# Bundle with esbuild for Lambda
esbuild dist/index.js --bundle --platform=node --target=node20 \
  --outfile=lambda/handler.js --external:@aws-sdk/*
```

### 3.4 Environment Variables

| Variable | Value | Source |
|----------|-------|--------|
| `PORT` | 4020 | Fixed |
| `SEPOLIA_RPC_URL` | Infura/Alchemy URL | AWS Secrets Manager |
| `CORS_ORIGINS` | `https://ultravioletadao.xyz` | Environment |

### 3.5 Cold Start Mitigation

The `fhevmInstance` creation during cold start adds 1-3s latency. Mitigations:

1. **Provisioned Concurrency**: Keep 1 warm instance
2. **Lazy initialization**: Already implemented in code
3. **Warming pings**: CloudWatch scheduled rule every 5 minutes

---

## 4. Phase 2: Facilitator Integration

### 4.1 Option A: Standalone (Recommended Initial)

No changes to x402-rs required. Clients interact directly:

- **Standard payments**: `https://facilitator.ultravioletadao.xyz`
- **FHE payments**: `https://zama-facilitator.ultravioletadao.xyz`

### 4.2 Option B: Proxy Integration (Future)

If desired, x402-rs can route FHE requests to the Lambda:

```rust
// src/chain/fhe.rs (new file)
pub async fn verify_fhe_payment(
    payload: &PaymentPayload,
    requirements: &PaymentRequirements,
) -> Result<VerifyResponse, FacilitatorError> {
    let client = reqwest::Client::new();
    let response = client
        .post("https://zama-facilitator.ultravioletadao.xyz/verify")
        .json(&VerifyRequest {
            payment_payload: payload.clone(),
            payment_requirements: requirements.clone(),
        })
        .send()
        .await?;

    response.json().await.map_err(|e| e.into())
}
```

Changes required:
1. Add `fhe-transfer` to `PaymentScheme` enum
2. Add proxy logic in `/verify` and `/settle` handlers
3. Add `ZAMA_FACILITATOR_URL` environment variable

### 4.3 Network Configuration

For the x402-rs facilitator to recognize the Zama-enabled network:

```rust
// src/network.rs - Add new network variant
#[serde(rename = "zama-sepolia")]
ZamaSepolia,

// Chain ID: 11155111 (same as Ethereum Sepolia)
// But uses FHE-enabled contracts
```

---

## 5. Phase 3: Testing & Validation

### 5.1 Unit Tests

```bash
# In x402-zama/packages/x402-facilitator
pnpm test
```

### 5.2 Integration Tests

Create test script at `tests/integration/test_zama_facilitator.py`:

```python
import requests

ZAMA_FACILITATOR_URL = "https://zama-facilitator.ultravioletadao.xyz"

def test_health():
    response = requests.get(f"{ZAMA_FACILITATOR_URL}/health")
    assert response.status_code == 200
    assert response.json()["status"] == "ok"

def test_verify_invalid():
    # Test with invalid payload
    response = requests.post(
        f"{ZAMA_FACILITATOR_URL}/verify",
        json={"invalid": "payload"}
    )
    assert response.status_code == 400
```

### 5.3 End-to-End Test Flow

1. Deploy ERC7984 test token on Sepolia
2. Mint tokens to test wallet
3. Generate decryption signature (client-side)
4. Execute `confidentialTransfer`
5. Call `/verify` with txHash + decryption signature
6. Verify response shows correct decrypted amount

---

## 6. Technical Details

### 6.1 x402-zama Facilitator API

#### `GET /health`
```json
{
  "status": "ok",
  "service": "x402-facilitator",
  "version": "1.0.0",
  "timestamp": "2024-01-15T10:30:00.000Z",
  "networks": ["sepolia"]
}
```

#### `POST /verify`

Request:
```json
{
  "x402Version": 1,
  "paymentPayload": {
    "x402Version": 1,
    "scheme": "fhe-transfer",
    "network": "sepolia",
    "chainId": 11155111,
    "payload": {
      "txHash": "0x...",
      "decryptionSignature": {
        "signature": "0x...",
        "publicKey": "0x...",
        "privateKey": "0x...",
        "userAddress": "0x...",
        "contractAddresses": ["0x..."],
        "startTimestamp": 1705312200,
        "durationDays": 365
      }
    }
  },
  "paymentRequirements": {
    "scheme": "fhe-transfer",
    "network": "sepolia",
    "chainId": 11155111,
    "payTo": "0x...",
    "maxAmountRequired": "1000000",
    "asset": "0x...",
    "resource": "https://example.com/api/premium",
    "description": "Premium content access",
    "mimeType": "application/json",
    "maxTimeoutSeconds": 300
  }
}
```

Response (Success):
```json
{
  "isValid": true,
  "txHash": "0x...",
  "amount": "1000000"
}
```

Response (Failure):
```json
{
  "isValid": false,
  "invalidReason": "Insufficient payment. Required: 1000000, Got: 500000"
}
```

### 6.2 Security Model

1. **Handle extraction from chain**: The facilitator fetches transaction data directly from the blockchain, not from client input
2. **Decryption authorization**: Users sign a message authorizing the facilitator to decrypt their amounts
3. **Scoped access**: Decryption signatures are valid only for specific contracts and have expiration
4. **ZAMA relayer trust**: The facilitator trusts ZAMA's relayer for decryption integrity

### 6.3 Dependencies

| Package | Version | Purpose |
|---------|---------|---------|
| `hono` | 4.6.0 | HTTP framework |
| `ethers` | 6.13.0 | Blockchain interaction |
| `@zama-fhe/relayer-sdk` | 0.3.0-5 | FHE operations |
| `zod` | 3.23.8 | Schema validation |

---

## 7. Cost Estimates

### 7.1 Lambda Costs

| Item | Monthly Estimate |
|------|------------------|
| Lambda invocations (10k/month) | $0.02 |
| Lambda duration (30s avg, 1GB) | $5.00 |
| Provisioned Concurrency (1 unit) | $8.00 |
| API Gateway requests | $0.50 |
| CloudWatch Logs | $1.00 |
| **Total** | **~$15/month** |

### 7.2 Comparison to ECS

The Lambda deployment is significantly cheaper than ECS for this use case:
- ECS Fargate: ~$40-50/month
- Lambda: ~$15/month

---

## 8. Risks & Mitigations

### 8.1 Technical Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Cold start latency (1-3s) | Poor UX | Provisioned Concurrency + warming pings |
| ZAMA relayer downtime | Service unavailable | Monitor + fallback messaging |
| FHE SDK changes | Breaking changes | Pin SDK version, test updates |
| RPC rate limits | Failed verifications | Premium RPC (Infura/Alchemy) |

### 8.2 Operational Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Testnet-only | Limited use cases | Monitor for mainnet availability |
| Limited networks | Only Sepolia | Expand when ZAMA adds networks |
| New technology | Integration bugs | Thorough testing, staged rollout |

---

## 9. Task Breakdown

### Phase 1: Lambda Infrastructure (Terraform)

| # | Task | Est. Effort | Dependencies |
|---|------|-------------|--------------|
| 1.1 | Create `terraform/environments/zama-testnet/` directory | 15 min | None |
| 1.2 | Write Lambda function Terraform resource | 1 hr | 1.1 |
| 1.3 | Write API Gateway HTTP API Terraform | 1 hr | 1.2 |
| 1.4 | Configure custom domain + ACM certificate | 30 min | 1.3 |
| 1.5 | Add Route53 DNS record | 15 min | 1.4 |
| 1.6 | Configure CloudWatch Logs | 15 min | 1.2 |
| 1.7 | Add IAM roles for Lambda | 30 min | 1.2 |
| 1.8 | Add Secrets Manager for RPC URL | 30 min | 1.2 |
| 1.9 | Add provisioned concurrency | 15 min | 1.2 |
| 1.10 | Create S3 bucket for Lambda deployment artifacts | 15 min | 1.1 |

### Phase 2: Lambda Code Deployment

| # | Task | Est. Effort | Dependencies |
|---|------|-------------|--------------|
| 2.1 | Fork x402-zama repo to UltravioletaDAO | 15 min | None |
| 2.2 | Configure Lambda adapter for Hono | 1 hr | 2.1 |
| 2.3 | Create esbuild bundle configuration | 30 min | 2.2 |
| 2.4 | Create CI/CD workflow (GitHub Actions) | 1 hr | 2.3 |
| 2.5 | Test Lambda locally with SAM CLI | 1 hr | 2.3 |
| 2.6 | Deploy to AWS | 30 min | 1.*, 2.* |
| 2.7 | Verify endpoints work | 30 min | 2.6 |

### Phase 3: Integration & Testing

| # | Task | Est. Effort | Dependencies |
|---|------|-------------|--------------|
| 3.1 | Create test ERC7984 token on Sepolia | 1 hr | None |
| 3.2 | Write integration test script | 1 hr | 2.7 |
| 3.3 | End-to-end payment test | 2 hr | 3.1, 3.2 |
| 3.4 | Add CloudWatch alarms | 30 min | 2.6 |
| 3.5 | Document deployment process | 1 hr | 3.3 |

### Phase 4: x402-rs Proxy Integration (ACTIVE - Option B)

> **Decision**: Single endpoint architecture. x402-rs proxies `fhe-transfer` requests to Lambda.

| # | Task | Est. Effort | Dependencies |
|---|------|-------------|--------------|
| 4.1 | Add `fhe-transfer` to `PaymentScheme` enum in `src/types.rs` | 30 min | 3.3 |
| 4.2 | Add FHE payload types (`FhePaymentPayload`, `DecryptionSignature`) | 1 hr | 4.1 |
| 4.3 | Add `ZAMA_FACILITATOR_URL` env var to `src/from_env.rs` | 30 min | 4.2 |
| 4.4 | Create `src/chain/fhe.rs` with proxy implementation | 2 hr | 4.3 |
| 4.5 | Update `/verify` handler to route `fhe-transfer` to proxy | 1 hr | 4.4 |
| 4.6 | Update `/settle` handler (FHE has no settle - return error) | 30 min | 4.5 |
| 4.7 | Update `/supported` endpoint to include `fhe-transfer` scheme | 30 min | 4.6 |
| 4.8 | Add Lambda URL to ECS task definition (Terraform) | 30 min | 4.7 |
| 4.9 | Integration test: full flow through x402-rs | 1 hr | 4.8 |

**Key Implementation Details**:

```rust
// src/chain/fhe.rs (new file)
pub async fn verify_fhe_payment(
    client: &reqwest::Client,
    lambda_url: &str,
    payload: &PaymentPayload,
    requirements: &PaymentRequirements,
) -> Result<VerifyResponse, FacilitatorError> {
    let response = client
        .post(format!("{}/verify", lambda_url))
        .json(&VerifyRequest {
            x402_version: 1,
            payment_payload: payload.clone(),
            payment_requirements: requirements.clone(),
        })
        .timeout(Duration::from_secs(35)) // Lambda timeout is 30s
        .send()
        .await?;

    response.json().await.map_err(|e| e.into())
}
```

### Total Estimated Effort (Full Scope - Option B)

| Phase | Original | Revised (with contingency) | Status |
|-------|----------|---------------------------|--------|
| Phase 1 (Infrastructure) | 5 hrs | 6-7 hrs | **COMPLETE** |
| Phase 2 (Code Deployment) | 5 hrs | 7-9 hrs | Pending |
| Phase 3 (Testing) | 5.5 hrs | 8-10 hrs | Pending |
| Phase 4 (Rust Integration) | 5 hrs | 6-8 hrs | **ACTIVE** |
| **Total** | **20.5 hrs** | **27-34 hrs** | |
| **With 50% contingency** | - | **40-51 hrs** | |

---

## Appendix A: Terraform Module Template (Complete)

```hcl
# terraform/environments/zama-testnet/main.tf

terraform {
  required_version = ">= 1.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = "us-east-2"
}

# Data Sources
data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

# Variables
variable "environment" {
  default = "testnet"
}

variable "cors_origins" {
  default = "https://ultravioletadao.xyz,http://localhost:3000"
}

variable "domain_name" {
  default = "zama-facilitator.ultravioletadao.xyz"
}

variable "hosted_zone_name" {
  default = "ultravioletadao.xyz"
}

# ============================================================================
# S3 Bucket for Lambda Artifacts
# ============================================================================

resource "aws_s3_bucket" "lambda_artifacts" {
  bucket = "zama-facilitator-artifacts-${data.aws_caller_identity.current.account_id}"
}

resource "aws_s3_bucket_versioning" "lambda_artifacts" {
  bucket = aws_s3_bucket.lambda_artifacts.id
  versioning_configuration {
    status = "Enabled"
  }
}

# ============================================================================
# Secrets Manager for RPC URL
# ============================================================================

resource "aws_secretsmanager_secret" "sepolia_rpc" {
  name = "zama-facilitator-sepolia-rpc"
}

# ============================================================================
# IAM Role for Lambda Execution
# ============================================================================

resource "aws_iam_role" "lambda_exec" {
  name = "zama-facilitator-lambda-${var.environment}"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = {
        Service = "lambda.amazonaws.com"
      }
    }]
  })
}

# CloudWatch Logs permissions
resource "aws_iam_role_policy_attachment" "lambda_logs" {
  role       = aws_iam_role.lambda_exec.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

# Secrets Manager permissions (CRITICAL - was missing)
resource "aws_iam_role_policy" "lambda_secrets" {
  name = "secrets-access"
  role = aws_iam_role.lambda_exec.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "secretsmanager:GetSecretValue",
        "secretsmanager:DescribeSecret"
      ]
      Resource = aws_secretsmanager_secret.sepolia_rpc.arn
    }]
  })
}

# ============================================================================
# CloudWatch Log Group
# ============================================================================

resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/zama-facilitator-${var.environment}"
  retention_in_days = 14
}

# ============================================================================
# Lambda Function
# ============================================================================

resource "aws_lambda_function" "zama_facilitator" {
  function_name = "zama-facilitator-${var.environment}"
  role          = aws_iam_role.lambda_exec.arn
  handler       = "handler.handler"
  runtime       = "nodejs20.x"
  memory_size   = 1024
  timeout       = 30

  s3_bucket = aws_s3_bucket.lambda_artifacts.id
  s3_key    = "handler.zip"

  environment {
    variables = {
      NODE_ENV     = "production"
      CORS_ORIGINS = var.cors_origins
    }
  }

  depends_on = [
    aws_cloudwatch_log_group.lambda,
    aws_iam_role_policy.lambda_secrets
  ]
}

# Provisioned Concurrency (mitigate cold starts)
resource "aws_lambda_provisioned_concurrency_config" "zama" {
  function_name                     = aws_lambda_function.zama_facilitator.function_name
  provisioned_concurrent_executions = 1
  qualifier                         = aws_lambda_function.zama_facilitator.version
}

# Lambda permission for API Gateway
resource "aws_lambda_permission" "api_gw" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.zama_facilitator.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.main.execution_arn}/*/*"
}

# ============================================================================
# API Gateway HTTP API
# ============================================================================

resource "aws_apigatewayv2_api" "main" {
  name          = "zama-facilitator-${var.environment}"
  protocol_type = "HTTP"

  cors_configuration {
    allow_origins = split(",", var.cors_origins)
    allow_methods = ["GET", "POST", "OPTIONS"]
    allow_headers = ["Content-Type", "Authorization", "x-payment"]
    max_age       = 300
  }
}

resource "aws_apigatewayv2_integration" "lambda" {
  api_id                 = aws_apigatewayv2_api.main.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.zama_facilitator.invoke_arn
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "default" {
  api_id    = aws_apigatewayv2_api.main.id
  route_key = "$default"
  target    = "integrations/${aws_apigatewayv2_integration.lambda.id}"
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.main.id
  name        = "$default"
  auto_deploy = true

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.api_gw.arn
    format = jsonencode({
      requestId      = "$context.requestId"
      ip             = "$context.identity.sourceIp"
      requestTime    = "$context.requestTime"
      httpMethod     = "$context.httpMethod"
      routeKey       = "$context.routeKey"
      status         = "$context.status"
      responseLength = "$context.responseLength"
      errorMessage   = "$context.error.message"
    })
  }
}

resource "aws_cloudwatch_log_group" "api_gw" {
  name              = "/aws/api-gw/zama-facilitator-${var.environment}"
  retention_in_days = 14
}

# ============================================================================
# Custom Domain (ACM + Route53)
# ============================================================================

data "aws_route53_zone" "main" {
  name         = var.hosted_zone_name
  private_zone = false
}

resource "aws_acm_certificate" "main" {
  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.main.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }

  allow_overwrite = true
  name            = each.value.name
  records         = [each.value.record]
  ttl             = 60
  type            = each.value.type
  zone_id         = data.aws_route53_zone.main.zone_id
}

resource "aws_acm_certificate_validation" "main" {
  certificate_arn         = aws_acm_certificate.main.arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]
}

resource "aws_apigatewayv2_domain_name" "main" {
  domain_name = var.domain_name

  domain_name_configuration {
    certificate_arn = aws_acm_certificate.main.arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }

  depends_on = [aws_acm_certificate_validation.main]
}

resource "aws_apigatewayv2_api_mapping" "main" {
  api_id      = aws_apigatewayv2_api.main.id
  domain_name = aws_apigatewayv2_domain_name.main.id
  stage       = aws_apigatewayv2_stage.default.id
}

resource "aws_route53_record" "main" {
  zone_id = data.aws_route53_zone.main.zone_id
  name    = var.domain_name
  type    = "A"

  alias {
    name                   = aws_apigatewayv2_domain_name.main.domain_name_configuration[0].target_domain_name
    zone_id                = aws_apigatewayv2_domain_name.main.domain_name_configuration[0].hosted_zone_id
    evaluate_target_health = false
  }
}

# ============================================================================
# AWS Budget Alert
# ============================================================================

resource "aws_budgets_budget" "zama_facilitator" {
  name         = "zama-facilitator-monthly"
  budget_type  = "COST"
  limit_amount = "20"
  limit_unit   = "USD"
  time_unit    = "MONTHLY"

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = 80
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_email_addresses = ["alerts@ultravioletadao.xyz"]
  }
}

# ============================================================================
# Outputs
# ============================================================================

output "api_gateway_url" {
  value = aws_apigatewayv2_api.main.api_endpoint
}

output "custom_domain_url" {
  value = "https://${var.domain_name}"
}

output "lambda_function_name" {
  value = aws_lambda_function.zama_facilitator.function_name
}

output "s3_bucket" {
  value = aws_s3_bucket.lambda_artifacts.id
}
```

---

## Appendix B: GitHub Actions Workflow Template

```yaml
# .github/workflows/deploy-zama-facilitator.yml
name: Deploy Zama Facilitator

on:
  push:
    branches: [main]
    paths:
      - 'packages/x402-facilitator/**'

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: pnpm/action-setup@v3
        with:
          version: 9

      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'pnpm'

      - run: pnpm install
      - run: pnpm --filter x402-facilitator build

      - name: Bundle for Lambda
        run: |
          cd packages/x402-facilitator
          npx esbuild dist/index.js \
            --bundle \
            --platform=node \
            --target=node20 \
            --outfile=lambda/handler.js \
            --external:@aws-sdk/*
          cd lambda && zip -r handler.zip .

      - name: Deploy to AWS
        uses: aws-actions/configure-aws-credentials@v4
        with:
          aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
          aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
          aws-region: us-east-2

      - name: Upload and update Lambda
        run: |
          aws s3 cp packages/x402-facilitator/lambda/handler.zip \
            s3://zama-facilitator-artifacts/handler.zip
          aws lambda update-function-code \
            --function-name zama-facilitator-production \
            --s3-bucket zama-facilitator-artifacts \
            --s3-key handler.zip
```

---

## Appendix C: References

- [x402-zama Repository](https://github.com/tomi204/x402-zama)
- [x402-zama Documentation](https://x402-fhe-docs.vercel.app/)
- [Zama FHEVM Documentation](https://docs.zama.org/protocol)
- [Zama Relayer SDK](https://docs.zama.org/protocol/relayer-sdk-guides)
- [ERC7984 Confidential Token Standard](https://eips.ethereum.org/EIPS/eip-7984)
- [x402 Protocol Specification](https://github.com/coinbase/x402)

---

## Appendix D: Task Decomposition Expert Review

**Review Date**: 2025-12-12
**Reviewer**: Task Decomposition Expert (aegis-rust-architect context)
**Project Phase**: Pre-execution planning review

### Executive Assessment

**Overall Rating**: 7.5/10 - Good foundation with critical gaps requiring attention

**Complexity**: Moderate-High (cross-technology integration, AWS Lambda, external dependencies)

**Estimated Effort**: Original estimate of 20.5 hours is **OPTIMISTIC**. Realistic estimate: **30-40 hours** including debugging, unknowns, and documentation.

**Recommended Agent Assignment**:
- **Phase 1 (Infrastructure)**: `terraform-aws-architect` - Lambda patterns differ significantly from ECS
- **Phase 2 (Code Deployment)**: Default agent (TypeScript bundling, no Rust involved)
- **Phase 3 (Testing)**: Default agent (Python integration tests)
- **Phase 4 (Rust Integration)**: `aegis-rust-architect` - Type system changes, scheme extensions

**Go/No-Go Decision**: CONDITIONAL GO - Address critical issues below before execution

---

### 1. Critical Issues (Must Fix Before Execution)

#### 1.1 MISSING: x402 v2 Protocol Compatibility Analysis

**CRITICAL BLOCKER**: The plan completely ignores that x402-rs just implemented v2 protocol support (v1.8.0, December 2024). The Zama integration must be evaluated against BOTH v1 and v2:

**Evidence from codebase**:
- `src/types.rs` now has `X402Version` enum supporting V1 and V2
- `src/types_v2.rs` exists with new `PaymentPayloadV2`, `ResourceInfo` structures
- `src/caip2.rs` provides CAIP-2 network identifier parsing
- Recent commits: "feat: Add x402 v2 types with CAIP-2 network support"

**Impact**:
1. Section 6.1 API examples use v1 format (`"network": "sepolia"`) but should also document v2 format (`"network": "eip155:11155111"`)
2. Task 4.1 "Add fhe-transfer scheme" needs to consider BOTH protocol versions
3. Integration testing (3.2-3.3) must verify v1 AND v2 request handling
4. The x402-zama TypeScript facilitator likely uses v1 only - needs verification

**Required Actions**:
- [ ] Review x402-zama repo to determine protocol version support
- [ ] Update Section 6.1 API examples to include v2 format
- [ ] Add task: "Verify x402-zama supports v2 protocol (or add v2 support)"
- [ ] Update integration tests to cover both v1 and v2 request formats
- [ ] Document CAIP-2 identifier for Sepolia FHE network (currently shows as `"sepolia"` string)

**Estimated Additional Effort**: +4-6 hours for v2 compatibility work

---

#### 1.2 MISSING: Hono-to-Lambda Adapter Configuration Details

**Issue**: Task 2.2 "Configure Lambda adapter for Hono" (1 hr) is severely underspecified. Hono is designed for edge/Node.js, NOT AWS Lambda by default.

**Current x402-zama code**: Uses `@hono/node-server` which is INCOMPATIBLE with Lambda's event model.

**Required Changes**:
1. Replace `@hono/node-server` with Lambda-compatible handler
2. Adapt Hono app to AWS Lambda event/context model
3. Handle API Gateway v2 payload format
4. Test local cold start behavior with SAM CLI

**Actual Implementation** (missing from plan):
```typescript
// lambda/handler.ts (NEW FILE REQUIRED)
import { Hono } from 'hono'
import { handle } from 'hono/aws-lambda'
import app from '../src/index' // Export the Hono app, don't call serve()

export const handler = handle(app)
```

**Required Actions**:
- [ ] Add task 2.1.5: "Refactor index.ts to export Hono app (don't call serve())"
- [ ] Add task 2.2.1: "Create lambda/handler.ts with hono/aws-lambda adapter"
- [ ] Add task 2.2.2: "Add @types/aws-lambda to devDependencies"
- [ ] Update esbuild bundle command to target `lambda/handler.ts`, not `dist/index.js`

**Estimated Additional Effort**: +2-3 hours (adapter research, testing, debugging)

---

#### 1.3 CRITICAL: Missing Secrets Manager IAM Permissions

**Issue**: Task 1.8 creates Secrets Manager secret but MISSING task to grant Lambda read access.

**Current plan**: Shows Lambda execution role (1.7) but doesn't specify permissions beyond CloudWatch Logs.

**Required IAM Policy** (missing from Appendix A):
```hcl
resource "aws_iam_role_policy" "lambda_secrets" {
  role = aws_iam_role.lambda_exec.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "secretsmanager:GetSecretValue",
        "secretsmanager:DescribeSecret"
      ]
      Resource = aws_secretsmanager_secret.sepolia_rpc.arn
    }]
  })
}
```

**Required Actions**:
- [ ] Add task 1.7.1: "Add Secrets Manager read permissions to Lambda IAM role"
- [ ] Update Appendix A Terraform template with complete IAM policy
- [ ] Add verification step in 2.7: "Test Lambda can read RPC URL from Secrets Manager"

**Impact if not fixed**: Lambda will fail at runtime with 403 Forbidden when accessing Secrets Manager.

---

#### 1.4 MISSING: Lambda Deployment Package Size Analysis

**Issue**: Task 2.3 bundles with esbuild but doesn't verify size constraints.

**AWS Lambda Limits**:
- Deployment package size: 50 MB (zipped), 250 MB (unzipped)
- Lambda layers: 5 layers max, 250 MB total unzipped

**Known Risk**: `@zama-fhe/relayer-sdk` may be large due to cryptographic libraries.

**Required Actions**:
- [ ] Add task 2.3.1: "Measure bundled handler.zip size"
- [ ] Add task 2.3.2: "If >10MB, investigate Lambda layers for @zama-fhe/relayer-sdk"
- [ ] Add contingency plan: "If package >50MB, use Docker image deployment (increases cold start)"

**Estimated Additional Effort**: +1-2 hours for size optimization

---

#### 1.5 MISSING: CORS Preflight Handling in Lambda

**Issue**: Section 3.2 shows CORS in API Gateway, but Lambda must also handle OPTIONS requests.

**Current Hono setup**: Has `cors()` middleware, but needs verification it returns correct headers for preflight.

**Required Testing**:
```bash
# Test CORS preflight
curl -X OPTIONS https://zama-facilitator.ultravioletadao.xyz/verify \
  -H "Origin: https://ultravioletadao.xyz" \
  -H "Access-Control-Request-Method: POST" \
  -v
```

**Required Actions**:
- [ ] Add task 2.2.3: "Verify Hono CORS middleware handles OPTIONS correctly"
- [ ] Add task 2.7.1: "Test CORS preflight from production domain"
- [ ] Document expected CORS headers in Section 6.1

---

### 2. Major Gaps (Should Address)

#### 2.1 Missing Error Handling Strategy

**Issue**: No tasks for error handling, retry logic, or circuit breakers.

**Zama Relayer Dependencies**:
- Relayer downtime (Section 8.1) has "Monitor + fallback messaging" but NO implementation
- No timeout configuration for relayer SDK calls
- No retry logic for transient RPC failures

**Required Tasks** (add to Phase 3):
| # | Task | Est. Effort |
|---|------|-------------|
| 3.6 | Implement timeout wrapper for Zama relayer SDK calls | 1 hr |
| 3.7 | Add exponential backoff for RPC failures | 1 hr |
| 3.8 | Create CloudWatch alarm for relayer error rate >10% | 30 min |

**Estimated Additional Effort**: +2.5 hours

---

#### 2.2 Missing Monitoring & Observability Tasks

**Issue**: Task 3.4 "Add CloudWatch alarms" is too vague. What metrics? What thresholds?

**Required Metrics** (missing from plan):
1. Lambda invocation errors (alarm if >5%)
2. Lambda duration (alarm if p99 >25s, approaching 30s timeout)
3. Cold start count (track provisioned concurrency effectiveness)
4. Zama relayer API errors (differentiate from user errors)
5. API Gateway 4xx vs 5xx errors

**Required Tasks**:
- [ ] 3.4.1: Add CloudWatch metric filter for relayer failures
- [ ] 3.4.2: Add SNS topic for alarm notifications
- [ ] 3.4.3: Add dashboard with Lambda duration, error rate, cold starts
- [ ] 3.4.4: Set up PagerDuty/email alerts for critical alarms

**Estimated Additional Effort**: +2-3 hours

---

#### 2.3 Missing Cost Monitoring & Budget Alerts

**Issue**: Section 7.1 estimates $15/month but no tasks to verify or alert on cost overruns.

**Risks**:
- Provisioned Concurrency ($8/month) is 53% of budget - easy to accidentally scale
- FHE operations may use more memory than 1GB (increases cost 2x if upgraded to 2GB)
- API Gateway request charges assume 10k/month - what if 100k?

**Required Tasks**:
- [ ] 1.11: Create AWS Budget with $20/month limit and 80% alert
- [ ] 3.9: Add CloudWatch dashboard showing daily Lambda cost
- [ ] 3.10: Document cost scaling scenarios (10x traffic, 100x traffic)

**Estimated Additional Effort**: +1 hour

---

#### 2.4 Network Naming Inconsistency: "sepolia" vs "ethereum-sepolia"

**CRITICAL DESIGN ISSUE**: The plan uses `"network": "sepolia"` throughout, but x402-rs uses `"ethereum-sepolia"` for Chain ID 11155111.

**Evidence from codebase**:
```rust
// src/network.rs:79
#[serde(rename = "ethereum-sepolia")]
EthereumSepolia,
```

**Impact on Phase 4 (x402-rs Integration)**:
- Task 4.2 suggests adding `ZamaSepolia` network variant
- This creates confusion: Is Zama on regular Sepolia or a separate network?
- RECOMMENDED: Reuse `EthereumSepolia` network, add FHE support via scheme detection

**Required Actions**:
- [ ] Update Section 4.3: Don't add new network variant, use existing `EthereumSepolia`
- [ ] Update Task 4.2: "Configure FHE contracts on EthereumSepolia network"
- [ ] Document in Section 6.1: FHE payments use `"network": "ethereum-sepolia"` (v1) or `"eip155:11155111"` (v2)
- [ ] Clarify: Zama uses standard Sepolia but with ERC7984 contracts (not a separate chain)

**Estimated Impact**: Simplifies Phase 4 by -30 min, improves clarity

---

### 3. Task Granularity Assessment

#### 3.1 Tasks That Are Too Coarse (Need Splitting)

**Task 1.2: "Write Lambda function Terraform resource" (1 hr)**
- TOO BROAD. Includes function, provisioned concurrency, permissions, environment variables.
- SPLIT INTO:
  - 1.2a: Define Lambda function resource (30 min)
  - 1.2b: Configure environment variables (15 min)
  - 1.2c: Add provisioned concurrency (moved from 1.9) (15 min)

**Task 2.2: "Configure Lambda adapter for Hono" (1 hr)**
- UNDERESTIMATED. Requires code changes, dependency updates, testing.
- SPLIT INTO:
  - 2.2a: Research hono/aws-lambda adapter pattern (30 min)
  - 2.2b: Refactor src/index.ts to export app (30 min)
  - 2.2c: Create lambda/handler.ts wrapper (30 min)
  - 2.2d: Test locally with sample Lambda event (30 min)
- REVISED ESTIMATE: 2 hours

**Task 3.3: "End-to-end payment test" (2 hr)**
- LACKS STRUCTURE. What are the sub-steps?
- SPLIT INTO:
  - 3.3a: Fund test wallet with Sepolia ETH (15 min)
  - 3.3b: Mint test ERC7984 tokens (30 min)
  - 3.3c: Generate FHE encryption keys (30 min)
  - 3.3d: Execute confidentialTransfer on-chain (30 min)
  - 3.3e: Call /verify with txHash (15 min)
  - 3.3f: Verify decrypted amount matches (15 min)
  - 3.3g: Document test results (15 min)
- REVISED ESTIMATE: 2.5 hours

---

#### 3.2 Tasks That Are Too Fine (Can Be Combined)

**Tasks 1.4 + 1.5: ACM Certificate + Route53 Record**
- These are tightly coupled (Route53 validates ACM via CNAME)
- COMBINE INTO: "1.4: Configure custom domain (ACM + Route53)" (45 min)

**Tasks 1.6 + 1.7: CloudWatch Logs + IAM Roles**
- IAM role creation includes CloudWatch permissions
- COMBINE INTO: "1.6: Configure IAM role with CloudWatch Logs access" (45 min)

**Revised Phase 1 Total**: Still ~5 hours (no net change, better organization)

---

#### 3.3 Missing Dependency Clarity

**Task 2.6: "Deploy to AWS" depends on "1.*, 2.*"**
- TOO VAGUE. What is the actual deployment order?
- CLARIFY DEPENDENCIES:
  1. Terraform apply (creates infrastructure) - MUST complete first
  2. Build Lambda bundle locally
  3. Upload to S3 (created by Terraform)
  4. Update Lambda function code
  5. Test Lambda execution

**Task 3.1: "Create test ERC7984 token" - No dependencies listed**
- INCORRECT. This should be done AFTER Lambda is deployed (Phase 2) so you can test with real facilitator.
- CORRECT DEPENDENCY: 2.7 (Verify endpoints work)

---

### 4. Effort Estimation Validation

#### 4.1 Comparison with Similar Projects

**Reference**: Adding NEAR Protocol support to x402-rs (v1.6.x)
- Required: New chain family (non-EVM), wallet management, meta-transactions
- Actual effort: ~15-20 hours (including debugging, testing, documentation)
- Plan estimated: Not documented, but comparable complexity

**Zama Integration Complexity**:
- LOWER: No Rust code for standalone Lambda (Phase 1-3)
- HIGHER: External service dependency (Zama relayer), new technology (FHE)
- SIMILAR: Terraform infrastructure, testing, integration

**Revised Estimates**:

| Phase | Original | Realistic | Contingency |
|-------|----------|-----------|-------------|
| Phase 1 (Infrastructure) | 5 hr | 6-7 hr | +1 hr for IAM debugging |
| Phase 2 (Code Deployment) | 5 hr | 7-9 hr | +2 hr for Hono adapter issues |
| Phase 3 (Testing) | 5.5 hr | 8-10 hr | +3 hr for ERC7984 token setup, FHE key generation |
| Phase 4 (Rust Integration) | 5 hr | 6-8 hr | +2 hr for v2 protocol compatibility |
| **TOTAL** | 20.5 hr | **27-34 hr** | **38-42 hr worst case** |

---

#### 4.2 Hidden Effort Not Captured in Tasks

**Missing Time Allocations**:
1. **Learning curve**: First time with Zama FHE, ERC7984 standard (+3-5 hr)
2. **Debugging cold starts**: Provisioned concurrency tuning (+1-2 hr)
3. **RPC endpoint setup**: Infura/Alchemy account, API key management (+1 hr)
4. **CI/CD debugging**: GitHub Actions secrets, AWS credentials, first-time deployment failures (+2-3 hr)
5. **Documentation**: Writing deployment guide, updating CLAUDE.md (+2 hr)

**Total Hidden Effort**: +9-13 hours

**FINAL REALISTIC ESTIMATE**: **36-47 hours** (vs. original 20.5 hours)

---

### 5. Architecture Decision Review

#### 5.1 Lambda vs ECS: Decision Soundness

**Evaluation Criteria**:

| Factor | Lambda (Plan) | ECS Fargate (Alternative) | Winner |
|--------|---------------|---------------------------|--------|
| **Cost** | ~$15/month | ~$40-50/month | Lambda |
| **Cold Starts** | 1-3s (mitigated by Provisioned Concurrency) | None | ECS |
| **Operational Complexity** | Lower (serverless) | Higher (container mgmt) | Lambda |
| **Technology Fit** | Node.js native support | Any runtime | Tie |
| **Consistency with x402-rs** | Different stack | Same AWS service | ECS |
| **Scaling** | Automatic, pay-per-use | Manual, always running | Lambda |
| **Debugging** | CloudWatch Logs only | SSH access, local Docker | ECS |

**VERDICT**: **Lambda is the correct choice** for initial deployment (Option A).

**Rationale**:
1. **Cost**: 3x cheaper for testnet-only, low-traffic use case
2. **FHE workload characteristics**: Infrequent, CPU-intensive bursts (Lambda optimized for this)
3. **Testnet experimentation**: Easy to tear down if Zama doesn't work out
4. **Decoupling**: Doesn't pollute x402-rs production environment

**Recommendation**: Proceed with Lambda (Option A), re-evaluate ECS if:
- Zama launches mainnet AND traffic >1000 requests/day
- Cold start latency becomes user complaint
- Need for persistent connections (unlikely for FHE)

---

#### 5.2 Standalone vs Proxy Integration

**Current Plan**: Option A (Standalone) initially, Option B (Proxy) later.

**CRITICAL INSIGHT**: Option B (Proxy) may be unnecessary complexity.

**Analysis**:
- **Client complexity**: Clients already need to know network + scheme. Knowing URL is equivalent effort.
- **Latency**: Proxy adds 50-200ms for no functional benefit.
- **Failure coupling**: x402-rs downtime would affect Zama payments (bad).
- **Protocol evolution**: x402 v2 supports multiple facilitators natively (clients choose).

**RECOMMENDATION**:
- **Commit to Option A (Standalone) permanently**.
- **Skip Phase 4 entirely** unless a specific use case emerges (e.g., unified billing).
- **Update `/supported` endpoint** in x402-rs to ADVERTISE Zama facilitator URL (don't proxy).

**Effort Savings**: -5 hours (skip Phase 4)

**Updated Total Estimate**: 31-42 hours (Phases 1-3 only, with revisions)

---

### 6. Risk Assessment Validation

#### 6.1 Risks Correctly Identified

**Well-Covered Risks**:
- Cold start latency (mitigated with Provisioned Concurrency)
- Zama relayer downtime (acknowledged, monitoring planned)
- Testnet-only limitation (accepted, documented)

#### 6.2 Missing Critical Risks

**RISK 1: Zama Relayer API Changes**
- **Probability**: Medium (SDK version 0.3.0-5 suggests active development)
- **Impact**: High (breaks payment verification)
- **Mitigation**:
  - Pin SDK version in package.json (ALREADY DONE)
  - Add integration test in CI that calls real Zama testnet (NEW TASK)
  - Subscribe to Zama SDK release notes

**RISK 2: Sepolia Testnet Resets**
- **Probability**: Low but non-zero (Sepolia replaced Ropsten/Rinkeby/Goerli)
- **Impact**: Critical (all test infrastructure lost)
- **Mitigation**:
  - Document all deployed contract addresses in DEPLOYMENT.md
  - Keep deployment scripts in repo
  - Monitor Ethereum Foundation announcements

**RISK 3: ERC7984 Standard Evolution**
- **Probability**: High (ERC7984 is draft, not final)
- **Impact**: High (changes to ConfidentialTransfer event structure)
- **Mitigation**:
  - Monitor EIP-7984 GitHub for changes
  - Test against reference implementation, not custom token
  - Document ERC7984 version used

**RISK 4: Lambda Provisioned Concurrency Cost Runaway**
- **Probability**: Medium (easy to misconfigure)
- **Impact**: Medium ($8/month becomes $80/month with 10 instances)
- **Mitigation**:
  - Set AWS Budget alert at $20/month
  - Use Terraform variable with default=1, max=2
  - Document in DEPLOYMENT.md: "DO NOT increase provisioned concurrency without cost approval"

**RISK 5: Decryption Signature Expiration**
- **Probability**: High (users may generate signatures and delay usage)
- **Impact**: Medium (poor UX, confusing errors)
- **Mitigation**:
  - Document signature TTL in client integration guide
  - Add helpful error message: "Decryption signature expired. Please regenerate."
  - Consider: Allow 7-day expiration for testnet (balance security vs UX)

**Add to Section 8**:
```markdown
### 8.3 Operational Risks (Additional)

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Zama SDK breaking changes | Medium | High | Pin version, integration tests, monitor releases |
| Sepolia testnet reset | Low | Critical | Document contracts, keep deployment scripts |
| ERC7984 standard evolution | High | High | Monitor EIP-7984 GitHub, use reference implementation |
| Provisioned Concurrency cost overrun | Medium | Medium | Budget alerts, Terraform limits, documentation |
| Decryption signature expiration | High | Medium | Clear error messages, document TTL |
```

---

### 7. Missing Tasks - Comprehensive List

**Add to Phase 1**:
- [ ] 1.2a: Define Lambda function resource (split from 1.2)
- [ ] 1.2b: Configure environment variables (split from 1.2)
- [ ] 1.2c: Add provisioned concurrency configuration (moved from 1.9)
- [ ] 1.7a: Add Secrets Manager read permissions to IAM policy
- [ ] 1.11: Create AWS Budget with $20/month alert

**Add to Phase 2**:
- [ ] 2.1a: Clone x402-zama repo to local /tmp
- [ ] 2.1b: Analyze package.json dependencies and bundle size
- [ ] 2.2a: Research hono/aws-lambda adapter (split from 2.2)
- [ ] 2.2b: Refactor src/index.ts to export Hono app (split from 2.2)
- [ ] 2.2c: Create lambda/handler.ts with adapter (split from 2.2)
- [ ] 2.2d: Verify CORS preflight handling
- [ ] 2.2e: Add @types/aws-lambda to devDependencies
- [ ] 2.3a: Measure bundled package size
- [ ] 2.3b: Verify size <50MB (contingency: use Docker image)
- [ ] 2.6a: Upload bundle to S3
- [ ] 2.6b: Update Lambda function code
- [ ] 2.6c: Verify Lambda execution with test event
- [ ] 2.7a: Test CORS preflight from allowed origin
- [ ] 2.7b: Test Lambda reads RPC URL from Secrets Manager

**Add to Phase 3**:
- [ ] 3.1a: Find or deploy reference ERC7984 implementation
- [ ] 3.1b: Verify ConfidentialTransfer event structure
- [ ] 3.3a: Fund test wallet with Sepolia ETH (split from 3.3)
- [ ] 3.3b: Mint test ERC7984 tokens (split from 3.3)
- [ ] 3.3c: Generate FHE encryption keys (split from 3.3)
- [ ] 3.3d: Execute confidentialTransfer (split from 3.3)
- [ ] 3.3e: Call /verify endpoint (split from 3.3)
- [ ] 3.3f: Verify decrypted amount (split from 3.3)
- [ ] 3.3g: Document test results (split from 3.3)
- [ ] 3.4a: Add metric filter for relayer failures
- [ ] 3.4b: Create SNS topic for alarms
- [ ] 3.4c: Add dashboard (duration, errors, cold starts)
- [ ] 3.4d: Configure alarm notifications
- [ ] 3.6: Implement timeout wrapper for relayer SDK
- [ ] 3.7: Add exponential backoff for RPC failures
- [ ] 3.8: Create CloudWatch alarm for relayer error rate
- [ ] 3.9: Add daily cost tracking dashboard
- [ ] 3.10: Document cost scaling scenarios

**Add to Phase 4 (if not skipped)**:
- [ ] 4.0: Analyze x402 v2 protocol compatibility
- [ ] 4.1a: Add FheTransfer variant to Scheme enum
- [ ] 4.1b: Add v2 payload structure for fhe-transfer
- [ ] 4.1c: Update VerifyRequest/SettleRequest deserialization
- [ ] 4.2: REVISED: Configure FHE contracts on EthereumSepolia (don't add new network)
- [ ] 4.5a: Test with v1 request format
- [ ] 4.5b: Test with v2 request format

**Documentation Tasks** (not in original plan):
- [ ] D.1: Update CLAUDE.md with Zama integration section
- [ ] D.2: Create ZAMA_DEPLOYMENT.md guide
- [ ] D.3: Update CHANGELOG.md with v1.9.0 entry
- [ ] D.4: Add Zama to README.md supported networks

---

### 8. Recommended Execution Strategy

#### 8.1 Pre-Execution Checklist (CRITICAL - DO BEFORE STARTING)

- [ ] **Review x402-zama repository thoroughly**
  - Verify it supports x402 v1 protocol (or v2)
  - Check for known issues, recent commits, maintenance status
  - Review test coverage and examples

- [ ] **Set up development environment**
  - Install SAM CLI for local Lambda testing
  - Configure AWS CLI with us-east-2 credentials
  - Install pnpm, Node.js 20.x

- [ ] **Validate Zama testnet availability**
  - Test relayer endpoint: `curl https://relayer.testnet.zama.cloud`
  - Review Zama documentation for any recent changes
  - Check Sepolia RPC endpoint (Infura/Alchemy)

- [ ] **Prepare AWS resources**
  - Verify Route53 hosted zone exists for ultravioletadao.xyz
  - Check ACM certificate quota (max 2048 per account)
  - Estimate current Lambda usage (free tier: 1M requests/month, 400,000 GB-seconds/month)

#### 8.2 Execution Phases (Revised)

**Week 1: Infrastructure + Code (Phases 1-2)**
- Day 1-2: Terraform infrastructure (Phase 1)
- Day 3-4: Lambda code adaptation (Phase 2)
- Day 5: Initial deployment + smoke tests

**Week 2: Testing + Hardening (Phase 3)**
- Day 1-2: ERC7984 token deployment, FHE key setup
- Day 3: End-to-end payment flow testing
- Day 4: Monitoring, alarms, cost tracking
- Day 5: Documentation, deployment guide

**Week 3 (Optional): Rust Integration (Phase 4)**
- Only if use case emerges for proxy integration
- Otherwise: SKIP and close out project

#### 8.3 Agent Handoff Strategy

**Phase 1 (Terraform)**:
1. **Start**: Request `terraform-aws-architect` agent
2. **Context**: Share this plan + review findings
3. **Deliverable**: Complete Terraform module in `terraform/environments/zama-testnet/`
4. **Handoff criterion**: `terraform apply` succeeds, Lambda function exists

**Phase 2 (TypeScript/Lambda)**:
1. **Start**: Default agent (no Rust code)
2. **Context**: Terraform outputs (Lambda ARN, S3 bucket, API Gateway URL)
3. **Deliverable**: Lambda function responding to test events
4. **Handoff criterion**: `curl` to API Gateway URL returns health check

**Phase 3 (Testing)**:
1. **Continue**: Default agent (Python integration tests)
2. **Context**: Deployed Lambda URL, ERC7984 contract address
3. **Deliverable**: Passing integration tests, monitoring dashboard
4. **Handoff criterion**: Full payment flow succeeds 3/3 times

**Phase 4 (If needed - Rust)**:
1. **Start**: Request `aegis-rust-architect` agent
2. **Context**: x402-rs codebase, Zama Lambda URL, API contract
3. **Deliverable**: x402-rs routes fhe-transfer to Lambda, tests pass
4. **Completion criterion**: `/supported` includes FHE, payment verification works end-to-end

---

### 9. Cost-Benefit Analysis

#### 9.1 Total Cost of Ownership (First Year)

| Category | Monthly | Annual | Notes |
|----------|---------|--------|-------|
| Lambda invocations | $0.02 | $0.24 | 10k requests/month |
| Lambda compute | $5.00 | $60.00 | 30s avg, 1GB RAM |
| Provisioned Concurrency | $8.00 | $96.00 | 1 instance 24/7 |
| API Gateway | $0.50 | $6.00 | HTTP API v2 |
| CloudWatch Logs | $1.00 | $12.00 | 14 day retention |
| Secrets Manager | $0.40 | $4.80 | 1 secret |
| Route53 queries | $0.10 | $1.20 | ~1k queries/month |
| **Subtotal** | **$15.02** | **$180.24** | Infrastructure |
| **Human effort** | - | **$3,600-$7,000** | 36-47 hrs at $100-150/hr |
| **Total Year 1** | - | **$3,780-$7,180** | |

**Break-even Analysis**:
- If project generates >$315-$600/month revenue → ROI positive in Year 1
- If testnet only (no revenue) → Sunk cost of $3,780-$7,180

#### 9.2 Value Proposition

**Pros**:
1. First FHE payment facilitator in x402 ecosystem (innovation leadership)
2. Privacy-preserving payments (unique selling point)
3. Low operational cost ($15/month after initial investment)
4. Learning experience with Zama FHE (strategic technology bet)

**Cons**:
1. Testnet only (no immediate revenue)
2. Single network (Sepolia only)
3. Dependency on external service (Zama relayer)
4. Unproven demand for FHE payments

**RECOMMENDATION**:
- **Proceed if**: Strategic interest in FHE, budget allows for R&D, potential future mainnet revenue
- **Defer if**: Limited budget, need immediate ROI, team lacks capacity for 36-47 hour project

---

### 10. Final Recommendations

#### 10.1 MUST DO Before Starting

1. **Update this plan** with:
   - x402 v2 protocol compatibility sections
   - Revised task breakdown (split coarse tasks)
   - Hono-Lambda adapter implementation details
   - Complete IAM permissions in Terraform template

2. **Validate Zama ecosystem**:
   - Clone x402-zama repo, build locally
   - Test against Zama testnet manually
   - Verify ERC7984 reference implementation exists

3. **Secure budget approval**:
   - Present realistic 36-47 hour estimate
   - Get sign-off on $180/year ongoing cost
   - Allocate contingency for unknowns

#### 10.2 SHOULD DO During Execution

1. **Use git worktrees** (per user's global instructions):
   ```bash
   git worktree add ../x402-rs-zama-integration feat/zama-integration
   cd ../x402-rs-zama-integration
   # Work here, merge to main when done
   ```

2. **Commit incrementally** (per user's preference):
   - After each task completion
   - Include timestamps in commit messages
   - Example: "feat: Add Lambda Terraform resources (2025-12-12 14:30)"

3. **Document as you go**:
   - Update CLAUDE.md immediately when architecture changes
   - Add entries to CHANGELOG.md after each phase
   - Screenshot and save all AWS console configurations

#### 10.3 COULD SKIP (Optional Optimizations)

1. **Phase 4 (Rust Integration)**: Recommend skipping entirely unless specific use case
2. **Provisioned Concurrency**: Start without it, add only if cold starts >3s
3. **Custom domain**: Use API Gateway default URL initially, add domain if production-ready

#### 10.4 WON'T DO (Out of Scope)

1. Mainnet deployment (Zama mainnet doesn't exist yet)
2. Multiple networks (Sepolia only for now)
3. Zama SDK modifications (use as-is)
4. Frontend integration (backend service only)

---

### 11. Approval Checklist

Before proceeding with execution, confirm:

- [x] Reviewed and understand all critical issues (Section 1) - **APPROVED 2025-12-12**
- [x] Accepted revised effort estimate of 27-34 hours (40-51 with contingency) - **APPROVED**
- [x] Budget approved for $180/year ongoing cost + development time - **APPROVED**
- [x] Decided on Phase 4: Skip or Execute? - **EXECUTE Option B (Proxy)**
- [x] Validated Zama testnet is operational - **VALIDATED**
- [x] Confirmed x402-zama repo is compatible with requirements - **v1 protocol compatible**
- [x] Assigned appropriate agents per phase - **terraform-aws-architect (P1), aegis-rust-architect (P4)**
- [x] Updated this plan document with findings from pre-execution research - **COMPLETE**

**Approval Status**: APPROVED - Ready for execution

---

**Review Completed**: 2025-12-12
**Approval Date**: 2025-12-12
**Architecture Changed**: 2025-12-12 - Switched from Option A (Standalone) to Option B (Proxy)
**Phase 1 Completed**: 2025-12-12 - Terraform infrastructure ready
**Next Action**: Phase 2 (Lambda code deployment)
**Scope**: Phases 1-4 (Full integration with single endpoint)
