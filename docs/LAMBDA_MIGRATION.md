# Lambda Migration Analysis: x402-rs Payment Facilitator

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Current Architecture (ECS Fargate)](#2-current-architecture-ecs-fargate)
3. [Lambda Architecture Proposal](#3-lambda-architecture-proposal)
4. [Cost Analysis](#4-cost-analysis)
5. [Technical Challenges](#5-technical-challenges)
6. [Migration Strategy](#6-migration-strategy)
7. [Risks and Mitigations](#7-risks-and-mitigations)
8. [Recommendation](#8-recommendation)
9. [Implementation Checklist](#9-implementation-checklist)

---

## 1. Executive Summary

This document evaluates migrating the x402-rs payment facilitator from AWS ECS Fargate
to AWS Lambda. The facilitator is a Rust/Axum HTTP service that processes gasless
micropayment settlements across 20+ blockchain networks (EVM, Solana, NEAR, Stellar,
Algorand, Sui, Fogo).

**TL;DR**: Lambda could reduce costs from ~$34/month to ~$1-5/month at current traffic
levels (low hundreds of requests per day), but introduces significant complexity around
cold starts, RPC connection caching, background tasks, and the OTel collector sidecar.
The recommended approach is the **Lambda Web Adapter** pattern, which wraps the existing
Axum binary with minimal code changes. Migration is worth pursuing only if the team
values cost reduction over operational simplicity, and should be staged carefully with a
canary deployment.

**When Lambda wins**: Low, bursty traffic (under 1,000 req/day) with tolerance for
occasional cold-start latency (200-800ms).

**When Fargate wins**: Sustained traffic over 5,000 req/day, latency-sensitive payment
flows, need for background tasks (discovery aggregation, crawling), or when operational
simplicity matters more than cost.

---

## 2. Current Architecture (ECS Fargate)

### Infrastructure Overview

```
Internet
   |
   v
Route53 (facilitator.ultravioletadao.xyz)
   |
   v
ALB (HTTPS termination, TLS 1.3)
   |
   +---> ECS Fargate Task (facilitator container, 1 vCPU / 2 GB)
   |        |
   |        +---> OTel Collector sidecar (Prometheus + Tempo)
   |
   +---> Lambda (balances API, via ALB listener rule at /api/balances)
   |
   v
Private Subnets --> NAT Gateway --> Internet (RPC endpoints)
```

### Key Components

| Component | Details |
|-----------|---------|
| **Compute** | ECS Fargate, 1 vCPU (1024 CPU units), 2 GB RAM |
| **Networking** | VPC with public/private subnets, NAT Gateway, ALB |
| **Container** | Multi-stage Dockerfile (rust:bullseye builder, debian:bullseye-slim runtime) |
| **Features** | `--features solana,near,stellar,algorand,sui` (all chains enabled) |
| **Secrets** | 14 wallet secrets + 2 RPC bundle secrets from AWS Secrets Manager |
| **Storage** | DynamoDB (nonce store), S3 (discovery persistence) |
| **Observability** | OTel Collector sidecar pushing to Prometheus + Tempo |
| **Auto-scaling** | 1-3 tasks, CPU target 75%, memory target 80% |
| **Health Check** | `curl -f http://localhost:8080/health` every 30s |

### Current Monthly Cost Breakdown

| Resource | Monthly Cost |
|----------|-------------|
| Fargate (1 vCPU, 2 GB, 24/7) | ~$18.40 |
| ALB (fixed hourly + LCU) | ~$16.20 |
| NAT Gateway (hourly + data) | ~$3.50 |
| CloudWatch Logs | ~$0.50 |
| Secrets Manager (16 secrets) | ~$6.40 |
| DynamoDB (on-demand, low usage) | ~$0.10 |
| S3 (discovery store, minimal) | ~$0.01 |
| **Total** | **~$45/month** |

[NOTE] The Fargate cost assumes 1 task running 24/7. The ALB is the single largest
fixed cost and would remain even with Lambda (unless replaced with API Gateway).

### Background Tasks

The facilitator runs several long-lived background tasks that are incompatible with
Lambda's request-response model:

1. **Discovery Aggregation** - Fetches resources from external facilitators every hour
   (configurable via `DISCOVERY_AGGREGATION_INTERVAL`)
2. **Discovery Crawler** - Crawls `/.well-known/x402` endpoints every 24 hours
   (configurable via `DISCOVERY_CRAWL_INTERVAL`)
3. **Self-Registration** - Registers the facilitator in the discovery registry at startup
4. **OTel Collector Sidecar** - Continuously receives and forwards telemetry data

---

## 3. Lambda Architecture Proposal

### 3.1 Lambda Web Adapter Approach (Recommended)

AWS Lambda Web Adapter (https://github.com/awslabs/aws-lambda-web-adapter) allows
running existing HTTP applications on Lambda without code changes. It works by:

1. Starting the Axum server inside the Lambda execution environment
2. Proxying Lambda invoke events as HTTP requests to localhost:8080
3. Returning the HTTP response as the Lambda response

```
API Gateway (HTTP API)
   |
   v
Lambda Function (Container Image)
   |
   +---> Lambda Web Adapter (extension layer)
   |        |
   |        +---> x402-rs Axum server (localhost:8080)
   |
   v
Internet (RPC endpoints via Lambda's VPC or public internet)
```

**Dockerfile changes required:**

```dockerfile
# Add Lambda Web Adapter
COPY --from=public.ecr.aws/awsguru/aws-lambda-web-adapter:0.8.4 \
     /lambda-adapter /opt/extensions/lambda-adapter

# Set adapter configuration
ENV AWS_LWA_PORT=8080
ENV AWS_LWA_READINESS_CHECK_PATH=/health
ENV AWS_LWA_READINESS_CHECK_MIN_UNHEALTHY_STATUS=500
```

**Pros:**
- Zero Rust code changes needed
- Same Docker image can run on both Fargate and Lambda
- Well-tested approach (official AWS solution)
- Maintains all existing behavior (CORS, OpenAPI, landing page)

**Cons:**
- Cold start includes full Axum boot (provider cache init, compliance checker, etc.)
- Background tasks (discovery aggregation/crawling) will not work
- OTel Collector sidecar is not available (must use Lambda OTel extension instead)

### 3.2 API Gateway Integration Options

#### Option A: HTTP API (Recommended)

| Feature | HTTP API (v2) | REST API (v1) |
|---------|--------------|---------------|
| **Cost** | $1.00/million requests | $3.50/million requests |
| **Latency** | Lower (no caching layer) | Higher |
| **Features** | JWT auth, CORS, OIDC | API keys, WAF, caching, usage plans |
| **WebSocket** | Supported | Not supported |
| **Custom domain** | Supported | Supported |
| **Payload limit** | 10 MB | 10 MB |

HTTP API is the clear choice: cheaper, lower latency, and has all features the
facilitator needs (CORS, custom domain, proxy integration).

**Configuration:**

```hcl
resource "aws_apigatewayv2_api" "facilitator" {
  name          = "facilitator-production"
  protocol_type = "HTTP"

  cors_configuration {
    allow_origins = ["*"]
    allow_methods = ["GET", "POST", "OPTIONS"]
    allow_headers = ["Content-Type", "Authorization", "X-Payment-*"]
    max_age       = 86400
  }
}
```

#### Option B: Keep ALB + Lambda Target Group

The facilitator already uses this pattern for the `/api/balances` Lambda. The main
facilitator Lambda could be added as another target group with weighted routing.

**Pros:** No DNS changes, gradual migration via ALB weighted rules.
**Cons:** Keeps ALB cost (~$16/month), reducing Lambda savings.

#### Option C: Lambda Function URL (Simplest)

Lambda Function URLs provide a dedicated HTTPS endpoint without API Gateway.

**Pros:** No API Gateway cost, simplest setup.
**Cons:** No custom domain without CloudFront, no request throttling, no WAF.

### 3.3 Container Image Lambda vs ZIP Deployment

| Factor | Container Image | ZIP Package |
|--------|----------------|-------------|
| **Max size** | 10 GB | 250 MB (50 MB zipped) |
| **x402-rs binary** | ~80-120 MB (estimated with all features) | Fits, but tight with config/static |
| **Build process** | Same Dockerfile, push to ECR | Cross-compile, package with bootstrap |
| **Cold start** | Slightly slower (image pull) | Slightly faster |
| **Caching** | ECR layer caching | Direct upload |
| **Recommendation** | **Use this** | Not recommended |

**Container Image is strongly recommended** because:

1. The existing Dockerfile already works (just add the web adapter layer)
2. The binary with all features (solana, near, stellar, algorand, sui) is large
3. Static assets (`static/index.html`, logos) and config files (`config/blacklist.json`)
   need to be included
4. Same image can be tested locally and deployed to both Fargate and Lambda

---

## 4. Cost Analysis

### Pricing Inputs (us-east-2, as of 2025)

| Resource | Price |
|----------|-------|
| Lambda compute (ARM64) | $0.0000133334/GB-second |
| Lambda compute (x86_64) | $0.0000166667/GB-second |
| Lambda requests | $0.20/million |
| API Gateway HTTP API | $1.00/million requests |
| Provisioned Concurrency | $0.0000041667/GB-second (idle) |
| Secrets Manager | $0.40/secret/month + $0.05/10K API calls |
| NAT Gateway | $0.045/hour + $0.045/GB data |

### Assumptions

- x86_64 architecture (Rust + Solana SDK constraints)
- Lambda memory: 1024 MB (balances runtime requirements for provider initialization)
- Average request duration: 500ms for verify, 2000ms for settle (RPC calls)
- Weighted average duration: 1000ms per request
- Secrets fetched once per cold start (cached in execution environment)
- Cold start rate: ~5% of invocations (Rust is fast, but container images are slower)

### Scenario 1: 100 Requests/Day (~Current Traffic)

**Lambda (HTTP API, no provisioned concurrency):**

| Item | Calculation | Monthly Cost |
|------|-------------|-------------|
| Lambda compute | 3,000 req x 1s x 1 GB x $0.0000166667 | $0.05 |
| Lambda requests | 3,000 x $0.20/1M | $0.00 |
| API Gateway | 3,000 x $1.00/1M | $0.00 |
| Secrets Manager | 16 secrets x $0.40 | $6.40 |
| DynamoDB | Same as current | $0.10 |
| S3 | Same as current | $0.01 |
| CloudWatch Logs | Reduced | $0.20 |
| **Total** | | **~$6.76/month** |

**Savings vs Fargate: ~$38/month (84% reduction)**

### Scenario 2: 1,000 Requests/Day

**Lambda (HTTP API, no provisioned concurrency):**

| Item | Calculation | Monthly Cost |
|------|-------------|-------------|
| Lambda compute | 30,000 req x 1s x 1 GB x $0.0000166667 | $0.50 |
| Lambda requests | 30,000 x $0.20/1M | $0.01 |
| API Gateway | 30,000 x $1.00/1M | $0.03 |
| Secrets Manager | 16 secrets x $0.40 | $6.40 |
| DynamoDB | Slightly higher | $0.20 |
| S3 | Same | $0.01 |
| CloudWatch Logs | | $0.50 |
| **Total** | | **~$7.65/month** |

**Savings vs Fargate: ~$37/month (83% reduction)**

### Scenario 3: 10,000 Requests/Day

**Lambda (HTTP API, no provisioned concurrency):**

| Item | Calculation | Monthly Cost |
|------|-------------|-------------|
| Lambda compute | 300,000 req x 1s x 1 GB x $0.0000166667 | $5.00 |
| Lambda requests | 300,000 x $0.20/1M | $0.06 |
| API Gateway | 300,000 x $1.00/1M | $0.30 |
| Secrets Manager | 16 secrets x $0.40 + API calls | $6.60 |
| DynamoDB | Higher throughput | $1.00 |
| S3 | Same | $0.01 |
| CloudWatch Logs | | $2.00 |
| **Total** | | **~$14.97/month** |

**Savings vs Fargate: ~$30/month (67% reduction)**

### Scenario 4: Lambda with Provisioned Concurrency

If cold starts are unacceptable for payment flows, provisioned concurrency keeps
instances warm:

| Provisioned Instances | Idle Cost/Month | Total (1K req/day) |
|-----------------------|-----------------|---------------------|
| 1 | $3.60 | ~$11.25 |
| 2 | $7.20 | ~$14.85 |
| 3 | $10.80 | ~$18.45 |

[NOTE] With 2+ provisioned instances, Lambda savings shrink significantly. At 3
provisioned instances, the cost advantage over Fargate narrows to ~$26/month.

### Break-Even Analysis

Lambda becomes more expensive than Fargate at approximately **50,000-60,000 requests/day**
(~1.5-1.8 million/month), assuming 1-second average duration and 1 GB memory. This is
well above the facilitator's expected traffic for the foreseeable future.

### Cost Summary Table

| Traffic Level | Fargate | Lambda | Lambda + 1 PC | Savings (Lambda) |
|--------------|---------|--------|---------------|------------------|
| 100 req/day | $45 | $7 | $11 | $34-38/month |
| 1,000 req/day | $45 | $8 | $12 | $33-37/month |
| 10,000 req/day | $45 | $15 | $19 | $26-30/month |
| 50,000 req/day | $45 | $52 | $56 | -$7 to -$11/month |

---

## 5. Technical Challenges

### 5.1 Cold Starts

**Current boot sequence** (from `src/main.rs`):

1. Load `.env` variables
2. Initialize OpenTelemetry (`Telemetry::new()`)
3. Build `ProviderCache::from_env()` -- iterates ALL 39 network variants, creates
   providers for each configured network (HTTP connections to 20+ RPC endpoints)
4. Initialize compliance checker (OFAC sanctions list + blacklist.json)
5. Create `FacilitatorLocal` with provider cache
6. Initialize discovery registry (optional S3 load)
7. Self-register in discovery
8. Start background aggregation/crawl tasks
9. Build Axum router and bind

**Estimated cold start breakdown:**

| Step | Time (estimated) |
|------|-----------------|
| Lambda container image pull + init | 500-1500ms |
| Rust binary start | <10ms |
| ProviderCache initialization (20+ networks) | 200-500ms |
| Compliance checker (OFAC list load) | 100-300ms |
| Discovery registry (S3 load) | 50-200ms |
| Axum router build | <10ms |
| Lambda Web Adapter health check | 100-500ms |
| **Total cold start** | **~1-3 seconds** |

[WARN] A 1-3 second cold start is problematic for payment settlements where clients
may have tight timeouts. However, Rust's cold start is still dramatically better than
Java (10-30s) or Python with heavy deps (3-10s).

**Mitigations:**
- Use provisioned concurrency (1 instance) to keep at least one warm
- Use ARM64 if Solana SDK supports it (faster cold starts)
- Lazy-initialize providers per-network on first use instead of all at startup
- Pre-warm with scheduled CloudWatch Events (ping /health every 5 minutes)

### 5.2 Provider Cache Invalidation

The `ProviderCache` in `src/provider_cache.rs` is a `HashMap<Network, NetworkProvider>`
built once at startup and shared across all requests via `Arc<FacilitatorLocal>`.

**On Lambda:**
- Each execution environment maintains its own provider cache (good for connection reuse)
- When an execution environment is recycled, all cached connections are lost
- Multiple concurrent Lambda instances each have their own cache (no sharing)
- This is functionally equivalent to the Fargate behavior (single instance)

**Impact:** Low. The provider cache works at the execution-environment level, and Lambda
execution environments are reused for sequential requests. The main cost is re-establishing
RPC connections after a cold start, which happens regardless of compute platform.

### 5.3 Connection Pooling to RPC Endpoints

The facilitator makes HTTP/HTTPS calls to 20+ blockchain RPC endpoints. On Fargate,
these connections benefit from HTTP keep-alive and connection pooling via the long-lived
process.

**On Lambda:**
- Warm invocations reuse the same TCP connections (execution environment persists)
- Cold starts require new connections to all RPC endpoints
- If the facilitator establishes connections eagerly during init (which it does via
  `ProviderCache::from_env()`), cold starts are slower but warm invocations are fast
- If connections are lazy, cold starts are faster but first requests to each chain are
  slower

**Impact:** Medium. For low traffic, most invocations may hit the same warm instance.
For bursty traffic (e.g., many concurrent settlements), multiple instances spin up and
each must establish fresh connections.

### 5.4 Lambda Timeout (15-Minute Maximum)

Lambda functions have a hard 15-minute timeout. The facilitator's operations are:

| Operation | Typical Duration | Max Duration |
|-----------|-----------------|-------------|
| `GET /health` | <10ms | <100ms |
| `GET /supported` | <50ms | <200ms |
| `POST /verify` | 100-500ms | 2-5s (slow RPC) |
| `POST /settle` | 500ms-5s | 30-60s (chain congestion) |
| `GET /` (landing page) | <10ms | <100ms |
| `GET /docs` (Swagger UI) | <50ms | <200ms |

**Impact:** Low. Even the slowest operation (settlement on a congested chain) completes
well within 15 minutes. The ALB idle timeout is currently 180 seconds; API Gateway HTTP
API has a 30-second integration timeout (configurable up to 30s for HTTP API, or 29s for
REST API).

[WARN] The API Gateway HTTP API has a **30-second maximum integration timeout**. If a
blockchain settlement takes longer than 30 seconds (rare but possible during extreme
congestion), the request will fail at the gateway level even though Lambda could handle
it. REST API has the same 29-second limit.

**Mitigations:**
- Implement async settlement: return a 202 Accepted with a tracking ID, then poll
- Use Lambda Function URL instead of API Gateway (no timeout limit except Lambda's 15min)
- Keep ALB integration (ALB timeout is configurable up to 4000 seconds)

### 5.5 Binary and Image Size

The x402-rs binary with all features (`solana,near,stellar,algorand,sui`) is substantial
due to cryptographic libraries, Solana SDK, Sui SDK (git dependency), and multiple chain
implementations.

| Item | Estimated Size |
|------|---------------|
| x402-rs release binary | 80-120 MB |
| Runtime dependencies (debian-slim + ca-certs + curl) | ~30 MB |
| Static assets (index.html + logos) | ~1 MB |
| Config files (blacklist.json) | <1 MB |
| Lambda Web Adapter | ~15 MB |
| **Total container image** | **~130-170 MB** |

**Lambda limits:**
- Container image: 10 GB maximum (well within limits)
- Ephemeral storage: 512 MB default (expandable to 10 GB)

**Impact:** Low. The image is well within Lambda container limits. Container image
cold starts are slightly slower than ZIP deployments, but the size is manageable.

### 5.6 Secrets Manager Integration

The facilitator loads 16+ secrets from AWS Secrets Manager at startup:

- 14 wallet private keys (EVM mainnet/testnet, Solana mainnet/testnet, NEAR, Stellar,
  Sui, Algorand -- 2 each, plus 2 legacy)
- 2 RPC URL bundles (mainnet and testnet premium endpoints)

**On Fargate (ECS):** Secrets are injected as environment variables by the ECS agent
before the container starts. The application reads them from `std::env`. No Secrets
Manager API calls from within the application.

**On Lambda:** Two options:

1. **Environment variables** (simpler, current approach equivalent):
   - Set secrets in Lambda environment variables (encrypted at rest)
   - Fetched once when the execution environment is created
   - [WARN] Environment variables have a 4 KB total size limit per function. With 16+
     secrets (private keys are ~64 hex chars each, RPC URLs vary), this could be tight.

2. **AWS Parameters and Secrets Lambda Extension** (recommended):
   - Caches secrets as a local HTTP endpoint (port 2773)
   - Fetched once per execution environment lifetime
   - Automatic rotation support
   - No 4 KB limit
   - Requires code changes to fetch from extension endpoint instead of env vars

3. **Direct SDK calls in init phase** (most flexible):
   - Call Secrets Manager API during Lambda INIT phase
   - Cache results in static/global variables
   - INIT phase has 10-second timeout (may be tight for 16 secret fetches)

**Recommended approach:** Use the Lambda extension for secrets, fetched during the INIT
phase. Alternatively, since the current code reads from environment variables
(`std::env::var`), set them directly in the Lambda configuration if within the 4 KB limit,
or use a custom init wrapper that fetches from Secrets Manager and populates env vars
before starting the Axum server.

### 5.7 Background Tasks

The facilitator runs several background tasks that are incompatible with Lambda:

| Task | Frequency | Purpose |
|------|-----------|---------|
| Discovery aggregation | Every 1 hour | Fetch resources from Coinbase and other facilitators |
| Discovery crawling | Every 24 hours | Crawl `.well-known/x402` endpoints |
| OTel Collector sidecar | Continuous | Forward traces/metrics to Prometheus + Tempo |

**On Lambda:**
- Background tasks cannot run (function freezes between invocations)
- OTel Collector sidecar does not exist (must use Lambda OTel extension)

**Mitigations:**
- Move discovery aggregation to a separate Lambda on a CloudWatch Events schedule
- Move discovery crawling to a separate scheduled Lambda
- Use AWS Distro for OpenTelemetry (ADOT) Lambda Layer for telemetry
- Accept that discovery data may be slightly stale between scheduled runs

### 5.8 DynamoDB and S3 Access

Both services are accessed via AWS SDK, which works identically on Lambda and Fargate.

- **DynamoDB (nonce store):** Lambda has native IAM role support. No VPC needed for
  DynamoDB access (uses AWS service endpoints).
- **S3 (discovery store):** Same as DynamoDB. No VPC needed.

**Impact:** None. These integrations work out of the box on Lambda.

### 5.9 VPC Considerations

The facilitator makes outbound HTTPS calls to blockchain RPC endpoints. On Fargate,
this goes through a NAT Gateway in the VPC.

**On Lambda (no VPC):**
- Lambda functions have internet access by default (no NAT Gateway needed)
- Direct access to DynamoDB, S3, Secrets Manager via public endpoints
- Saves NAT Gateway cost (~$3.50/month)
- [NOTE] If DynamoDB/S3 use VPC endpoints, Lambda must be in the VPC

**On Lambda (with VPC):**
- Required only if accessing resources in the VPC (e.g., ElastiCache, RDS)
- The facilitator does not use any VPC-internal resources
- VPC Lambda adds cold start latency (ENI attachment: 1-2s, mitigated by Hyperplane)

**Recommendation:** Run Lambda **without VPC** attachment. All external services
(RPC endpoints, DynamoDB, S3, Secrets Manager) are accessible over the public internet
or via AWS service endpoints.

---

## 6. Migration Strategy

### Phase 1: Lambda Web Adapter (Weeks 1-2)

**Goal:** Run the existing Axum binary on Lambda with minimal changes.

**Steps:**

1. Create a new Dockerfile variant (`Dockerfile.lambda`):
   ```dockerfile
   FROM <existing-builder-stage>

   FROM public.ecr.aws/lambda/provided:al2023
   COPY --from=builder /app/target/release/x402-rs /usr/local/bin/x402-rs
   COPY --from=builder /app/config /app/config
   COPY --from=builder /app/static /app/static
   COPY --from=public.ecr.aws/awsguru/aws-lambda-web-adapter:0.8.4 \
        /lambda-adapter /opt/extensions/lambda-adapter

   ENV AWS_LWA_PORT=8080
   ENV AWS_LWA_READINESS_CHECK_PATH=/health
   WORKDIR /app
   ENTRYPOINT ["x402-rs"]
   ```

2. Create Terraform for Lambda function:
   - Container image from ECR
   - 1024 MB memory, 60-second timeout
   - IAM role with Secrets Manager, DynamoDB, S3 access
   - Environment variables for configuration

3. Create API Gateway HTTP API:
   - Routes: `GET /`, `GET /health`, `GET /supported`, `POST /verify`, `POST /settle`,
     `GET /docs`, `GET /api-docs/openapi.json`
   - Custom domain: `facilitator-lambda.ultravioletadao.xyz` (staging)

4. Disable background tasks via env vars:
   - `DISCOVERY_ENABLE_AGGREGATION=false`
   - `DISCOVERY_ENABLE_CRAWLER=false`

5. Test all endpoints against the Lambda deployment.

**Exit criteria:** All API endpoints return identical responses to Fargate.

### Phase 2: Optimize for Lambda (Weeks 3-4)

**Goal:** Reduce cold start time and improve Lambda-specific behavior.

**Steps:**

1. **Lazy provider initialization:**
   - Modify `ProviderCache::from_env()` to initialize providers lazily (on first use
     per network) instead of eagerly for all 39 variants
   - This reduces cold start from ~500ms to ~50ms for provider init
   - First request to each network pays the connection cost

2. **Secrets Manager optimization:**
   - Add AWS Parameters and Secrets Lambda Extension
   - Or: fetch all secrets in a single batch during INIT phase

3. **OTel Lambda integration:**
   - Replace OTel Collector sidecar with ADOT Lambda Layer
   - Configure `OTEL_EXPORTER_OTLP_ENDPOINT` to point to ADOT

4. **Background tasks as separate Lambdas:**
   - Create `facilitator-discovery-aggregator` Lambda (CloudWatch Events, hourly)
   - Create `facilitator-discovery-crawler` Lambda (CloudWatch Events, daily)
   - These can be lightweight Python functions or minimal Rust binaries

5. **Response streaming (optional):**
   - For the landing page (`GET /`), use Lambda response streaming to reduce TTFB

### Phase 3: Canary Deployment (Weeks 5-6)

**Goal:** Gradually shift traffic from Fargate to Lambda.

**Option A: ALB Weighted Routing (Recommended)**

Since the ALB already exists and supports Lambda target groups (proven with
`/api/balances`), use weighted target group routing:

```hcl
# ALB listener rule with weighted targets
resource "aws_lb_listener_rule" "canary" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 5

  action {
    type = "forward"

    forward {
      target_group {
        arn    = aws_lb_target_group.main.arn       # Fargate
        weight = 90
      }
      target_group {
        arn    = aws_lb_target_group.lambda.arn      # Lambda
        weight = 10
      }
    }
  }

  condition {
    path_pattern {
      values = ["/*"]
    }
  }
}
```

Gradually shift: 10% -> 25% -> 50% -> 75% -> 100%.

**Option B: Route53 Weighted Routing**

Use Route53 weighted records to split traffic between ALB (Fargate) and API Gateway
(Lambda):

```hcl
resource "aws_route53_record" "fargate" {
  zone_id        = data.aws_route53_zone.main.zone_id
  name           = "facilitator.ultravioletadao.xyz"
  type           = "A"
  set_identifier = "fargate"

  weighted_routing_policy {
    weight = 90
  }

  alias {
    name    = aws_lb.main.dns_name
    zone_id = aws_lb.main.zone_id
  }
}

resource "aws_route53_record" "lambda" {
  zone_id        = data.aws_route53_zone.main.zone_id
  name           = "facilitator.ultravioletadao.xyz"
  type           = "A"
  set_identifier = "lambda"

  weighted_routing_policy {
    weight = 10
  }

  alias {
    name    = aws_apigatewayv2_domain_name.facilitator.domain_name_configuration[0].target_domain_name
    zone_id = aws_apigatewayv2_domain_name.facilitator.domain_name_configuration[0].hosted_zone_id
  }
}
```

**Monitoring during canary:**
- Compare p50/p95/p99 latency between Fargate and Lambda
- Monitor cold start frequency and duration
- Track settlement success rates per platform
- Watch for timeout errors on settlement requests

### Phase 4: Full Migration (Week 7+)

**Goal:** Decommission Fargate infrastructure.

**Steps:**

1. Route 100% traffic to Lambda
2. Monitor for 1 week with zero Fargate traffic
3. Tear down Fargate resources:
   - ECS Service and Cluster
   - Fargate task definition
   - NAT Gateway (if Lambda is VPC-less)
   - Private subnets route tables
4. Keep ALB if using ALB + Lambda Target Group approach
5. Or: Remove ALB if using API Gateway + custom domain

---

## 7. Risks and Mitigations

### Risk Matrix

| Risk | Severity | Likelihood | Mitigation |
|------|----------|-----------|------------|
| Cold start delays payment settlement | High | Medium | Provisioned concurrency (1 instance); pre-warming via scheduled pings |
| API Gateway 30s timeout kills slow settlements | High | Low | Use ALB + Lambda Target Group instead of API Gateway; or implement async settlement |
| Secrets Manager 4 KB env var limit exceeded | Medium | Medium | Use Lambda Secrets Extension; or fetch via SDK in init phase |
| Background tasks (discovery) stop working | Medium | Certain | Implement as separate scheduled Lambdas |
| OTel telemetry gaps | Medium | Medium | Use ADOT Lambda Layer; accept some trace loss during cold starts |
| Concurrent execution creates many RPC connections | Medium | Low | Most RPC providers handle this; monitor rate limiting |
| Container image cold start slower than expected | Medium | Low | Optimize image size; strip debug symbols; use `lto = true` in release profile |
| Lambda execution environment recycling loses in-memory discovery cache | Low | Medium | Rely on S3 persistence; accept slightly stale data |
| Cost increases if traffic spikes unexpectedly | Low | Low | Set Lambda reserved concurrency limit; monitor billing alerts |

### Rollback Plan

At every phase, the original Fargate infrastructure remains intact:

1. **Phase 1-2:** Lambda runs on a separate domain (`facilitator-lambda.ultravioletadao.xyz`).
   Rollback = do nothing (Fargate is still primary).

2. **Phase 3:** ALB weighted routing. Rollback = set Fargate weight to 100%.
   Single Terraform change, applies in seconds.

3. **Phase 4:** If Fargate is decommissioned, re-deploy from the same Docker image
   and Terraform config (kept in version control). Recovery time: ~15 minutes.

---

## 8. Recommendation

### Decision Framework

| Factor | Fargate Wins | Lambda Wins |
|--------|-------------|-------------|
| **Cost at current traffic** | | X (saves ~$35/month) |
| **Operational simplicity** | X | |
| **Cold start sensitivity** | X (always warm) | |
| **Background tasks** | X (native support) | |
| **Scaling to zero** | | X (no idle cost) |
| **Burst scaling** | | X (instant, no task spin-up) |
| **OTel integration** | X (sidecar pattern) | |
| **Settlement reliability** | X (no timeout limits) | |
| **Team familiarity** | X (current setup) | |

### Verdict

**For the current state of the facilitator: Stay on Fargate.**

The savings of ~$35/month ($420/year) do not justify the migration effort and
operational complexity, given:

1. The facilitator handles **financial transactions** where cold-start latency and
   timeout risks directly impact user experience and payment reliability.
2. Background tasks (discovery aggregation/crawling) would need to be re-architected
   as separate Lambda functions.
3. The OTel Collector sidecar (Prometheus + Tempo integration) would need replacement
   with ADOT Lambda Layer, a different operational model.
4. The team would need to maintain two deployment models during transition.

### When to Reconsider Lambda

Lambda becomes the right choice when ANY of these conditions are true:

1. **Traffic drops to near-zero** for extended periods (facilitator is idle most hours)
   and the $45/month fixed cost matters.
2. **Multiple services share the ALB** -- the ALB cost gets amortized and Lambda's
   per-request cost is genuinely cheaper.
3. **The discovery background tasks are moved to a separate service** -- removing the
   last reason the facilitator needs a long-running process.
4. **AWS introduces Lambda SnapStart for container images** -- reducing cold starts
   to <200ms, eliminating the primary technical concern.
5. **The facilitator is split into microservices** -- verify and settle become
   separate Lambdas, each optimized for their workload.

### Alternative: Hybrid Approach

A pragmatic middle ground that captures some Lambda savings without full migration:

1. **Keep Fargate for the main facilitator** (verify, settle, discovery, OTel)
2. **Move read-only endpoints to Lambda** (already done for `/api/balances`):
   - `GET /supported` -- static data, perfect for Lambda
   - `GET /` -- landing page, cacheable
   - `GET /docs` -- Swagger UI, static
3. **Reduce Fargate size** to 0.5 vCPU / 1 GB if traffic allows (saves ~$9/month)

This hybrid approach is already partially implemented (the balances Lambda exists) and
requires no changes to the core payment flow.

---

## 9. Implementation Checklist

If the decision is made to proceed with Lambda migration, follow this checklist:

### Phase 1: Lambda Web Adapter

- [ ] Create `Dockerfile.lambda` based on existing Dockerfile
- [ ] Add Lambda Web Adapter extension layer
- [ ] Create ECR repository for Lambda image (or reuse existing `facilitator` repo with
      different tags)
- [ ] Create Terraform module: `terraform/environments/production/lambda-facilitator.tf`
  - [ ] Lambda function resource (container image, 1024 MB, 60s timeout)
  - [ ] IAM execution role with policies for:
    - [ ] Secrets Manager access (all 16 secrets)
    - [ ] DynamoDB access (nonce store)
    - [ ] S3 access (discovery store)
    - [ ] CloudWatch Logs
  - [ ] Lambda environment variables (all from current ECS task definition)
  - [ ] API Gateway HTTP API (or ALB target group)
  - [ ] Custom domain configuration
  - [ ] CloudWatch Log Group
- [ ] Disable background tasks in Lambda config:
  - [ ] `DISCOVERY_ENABLE_AGGREGATION=false`
  - [ ] `DISCOVERY_ENABLE_CRAWLER=false`
- [ ] Build and push Lambda container image
- [ ] Test all endpoints:
  - [ ] `GET /` returns landing page with Ultravioleta branding
  - [ ] `GET /health` returns `{"status":"healthy"}`
  - [ ] `GET /supported` returns all networks
  - [ ] `POST /verify` with test payload
  - [ ] `POST /settle` with test payload (testnet only)
  - [ ] `GET /docs` returns Swagger UI
  - [ ] `GET /api-docs/openapi.json` returns OpenAPI spec
- [ ] Measure cold start time (target: <2 seconds)
- [ ] Measure warm invocation latency for each endpoint

### Phase 2: Optimization

- [ ] Implement lazy provider initialization in `ProviderCache`
- [ ] Add ADOT Lambda Layer for OpenTelemetry
- [ ] Create scheduled Lambda for discovery aggregation (hourly)
- [ ] Create scheduled Lambda for discovery crawling (daily)
- [ ] Evaluate provisioned concurrency need based on Phase 1 metrics
- [ ] If needed, configure provisioned concurrency = 1

### Phase 3: Canary

- [ ] Add Lambda target group to ALB
- [ ] Configure ALB weighted routing (10% Lambda / 90% Fargate)
- [ ] Create CloudWatch dashboard comparing Lambda vs Fargate:
  - [ ] P50/P95/P99 latency
  - [ ] Error rates
  - [ ] Cold start frequency
  - [ ] Settlement success rate
- [ ] Gradually increase Lambda weight: 25% -> 50% -> 75% -> 100%
- [ ] Monitor for 1 week at each increment

### Phase 4: Decommission Fargate

- [ ] Confirm 100% traffic on Lambda for 7+ days with no issues
- [ ] Remove ECS service, task definition, and cluster
- [ ] Remove NAT Gateway (if Lambda is VPC-less)
- [ ] Remove ECS-specific security groups
- [ ] Update Route53 records (if switching from ALB to API Gateway)
- [ ] Archive Fargate Terraform configs (do not delete, keep for rollback reference)
- [ ] Update CLAUDE.md and README.md to reflect new architecture
- [ ] Update deployment scripts (`scripts/build-and-push.sh`)
- [ ] Update monitoring and alerting

---

## Appendix A: Lambda Web Adapter Reference

- **Repository:** https://github.com/awslabs/aws-lambda-web-adapter
- **ECR Image:** `public.ecr.aws/awsguru/aws-lambda-web-adapter:0.8.4`
- **Supported frameworks:** Axum, Actix, Rocket, Warp, and any HTTP server
- **Configuration:**
  - `AWS_LWA_PORT` - Port your app listens on (default: 8080)
  - `AWS_LWA_READINESS_CHECK_PATH` - Health check path (default: `/`)
  - `AWS_LWA_READINESS_CHECK_MIN_UNHEALTHY_STATUS` - Min status code considered unhealthy (default: 500)
  - `AWS_LWA_ASYNC_INIT` - Enable async init for long-starting apps (default: false)

## Appendix B: Relevant AWS Pricing Pages

- Lambda pricing: https://aws.amazon.com/lambda/pricing/
- API Gateway pricing: https://aws.amazon.com/api-gateway/pricing/
- Fargate pricing: https://aws.amazon.com/fargate/pricing/
- Secrets Manager pricing: https://aws.amazon.com/secrets-manager/pricing/
- NAT Gateway pricing: https://aws.amazon.com/vpc/pricing/

## Appendix C: Related Files in This Repository

| File | Relevance |
|------|-----------|
| `src/main.rs` | Startup sequence, background tasks, Axum router construction |
| `src/provider_cache.rs` | RPC connection caching, eager initialization of all networks |
| `src/from_env.rs` | Environment variable definitions for secrets and RPC URLs |
| `src/handlers.rs` | All HTTP endpoint handlers |
| `src/nonce_store.rs` | DynamoDB nonce storage |
| `src/discovery_store.rs` | S3 discovery persistence |
| `src/escrow.rs` | Escrow settlement (requires ENABLE_ESCROW=true) |
| `src/telemetry.rs` | OpenTelemetry initialization |
| `Dockerfile` | Current multi-stage build |
| `terraform/environments/production/main.tf` | ECS Fargate infrastructure |
| `terraform/environments/production/secrets.tf` | Secrets Manager references |
| `terraform/environments/production/lambda-balances.tf` | Existing Lambda pattern (reference) |

---

*Document created: 2026-02-09*
*Last updated: 2026-02-09*
*Author: Infrastructure team, Ultravioleta DAO*
