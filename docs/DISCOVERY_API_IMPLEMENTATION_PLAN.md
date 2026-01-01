# Discovery API (Bazaar) Implementation Plan

## Executive Summary

This document outlines the plan to enable and make public the x402 Discovery API (Bazaar) for the Ultravioleta DAO Payment Facilitator. The Discovery API allows paid services to register themselves, enabling clients and AI agents to automatically discover available x402-enabled endpoints.

## Current State Analysis

### What's Already Implemented

The discovery feature is **fully implemented in the codebase** but **not deployed to production**:

| Component | Location | Status |
|-----------|----------|--------|
| Discovery Registry | `src/discovery.rs` | Complete |
| Storage Backends | `src/discovery_store.rs` | S3, Memory, NoOp |
| HTTP Handlers | `src/handlers.rs:119-321` | Complete |
| Route Mounting | `src/main.rs:195` | Complete |
| V2 Types | `src/types_v2.rs:895-1104` | Complete |
| Landing Page Docs | `static/index.html:1997-2024` | Complete |
| Self-Registration | `src/main.rs:147-185` | Complete |

### Production Gap

| Item | Local (v1.17.0) | Production (v1.17.2 image) |
|------|-----------------|---------------------------|
| Version endpoint | 1.17.0 | 1.17.0 (but code mismatch) |
| Discovery Endpoints | Present | Working (GET /discovery/resources returns valid response) |
| /supported extensions | `["bazaar"]` | **MISSING** - returns only `kinds` |
| Discovery Registry Init | Logs "Initializing Bazaar..." | **NOT LOGGED** at startup |
| Self-Registration | Via FACILITATOR_URL | **NOT WORKING** - no log, empty registry |
| S3 Persistence | Supported | Not configured (DISCOVERY_S3_BUCKET not set) |

### Root Cause Analysis

**Problem**: The deployed Docker image v1.17.2 was built from code that doesn't properly include the discovery features:

1. **Evidence 1**: Startup logs show NO "Initializing Bazaar discovery registry..." message
2. **Evidence 2**: `/supported` endpoint returns v1 format (no `extensions` field)
3. **Evidence 3**: `/discovery/resources` returns empty items (self-registration not working)

**Likely Cause**: Docker build cache issue or the image was built from uncommitted changes that didn't include the full discovery integration.

**Solution**: Rebuild Docker image from current HEAD (which contains all discovery code) and redeploy.

---

## x402 Discovery API Specification

### Protocol Overview

From the [x402 V2 specification](https://www.x402.org/writing/x402-v2-launch):

> "The Discovery extension creates a more autonomous ecosystem in which sellers publish their APIs once, and facilitators stay synchronized without developer intervention."

### Key Concepts

1. **Bazaar**: A discovery registry where x402-enabled services register themselves
2. **Resource Types**: `http` (APIs), `mcp` (Model Context Protocol), `a2a` (Agent-to-Agent), `facilitator` (payment processors)
3. **Automatic Crawling**: Facilitators can automatically index available endpoints and pricing

### Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/discovery/resources` | GET | List registered paid resources with pagination and filtering |
| `/discovery/register` | POST | Register a new paid resource |
| `/supported` | GET | List supported networks with `extensions` field including "bazaar" |

---

## Implementation Plan

### Phase 1: Fix Production Deployment (CRITICAL)

**Objective**: Rebuild and deploy with proper discovery features

The current v1.17.2 image is missing discovery features. We need to rebuild from HEAD.

#### 1.1 Verify Local Code Has Discovery Features

```bash
# Check that discovery init is present in main.rs
grep "Initializing Bazaar" src/main.rs
# Expected: tracing::info!("Initializing Bazaar discovery registry...");

# Check that /supported uses v2 format with extensions
grep -A10 "let v2_response = supported.to_v2" src/handlers.rs
```

#### 1.2 Build Fresh Docker Image (No Cache)

```bash
# Clean build to avoid cache issues
docker build --no-cache -t facilitator:1.18.0 .

# Or use the build script
./scripts/build-and-push.sh 1.18.0
```

#### 1.3 Deploy to ECS

```bash
# Update ECS service with new image
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

#### 1.3 Verify Discovery Endpoints

```bash
# Test discovery endpoints after deployment
curl https://facilitator.ultravioletadao.xyz/discovery/resources
curl https://facilitator.ultravioletadao.xyz/supported | jq '.extensions'
```

**Expected /supported Response with Extensions**:
```json
{
  "kinds": [...],
  "extensions": ["bazaar"],
  "signers": {}
}
```

---

### Phase 2: S3 Persistence Configuration

**Objective**: Enable persistent storage so registrations survive restarts

#### 2.1 Create S3 Bucket

```bash
aws s3 mb s3://facilitator-discovery-prod --region us-east-2

# Enable versioning for recovery
aws s3api put-bucket-versioning \
  --bucket facilitator-discovery-prod \
  --versioning-configuration Status=Enabled
```

#### 2.2 Update Terraform Configuration

Add to `terraform/environments/production/main.tf`:

```hcl
# S3 bucket for Discovery API persistence
resource "aws_s3_bucket" "discovery" {
  bucket = "facilitator-discovery-prod"

  tags = {
    Name        = "Discovery API Storage"
    Environment = "production"
  }
}

resource "aws_s3_bucket_versioning" "discovery" {
  bucket = aws_s3_bucket.discovery.id
  versioning_configuration {
    status = "Enabled"
  }
}

# IAM policy for ECS task to access S3
resource "aws_iam_role_policy" "discovery_s3" {
  name = "discovery-s3-access"
  role = aws_iam_role.ecs_task_execution.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ]
        Resource = [
          aws_s3_bucket.discovery.arn,
          "${aws_s3_bucket.discovery.arn}/*"
        ]
      }
    ]
  })
}
```

#### 2.3 Add Environment Variables to Task Definition

In ECS task definition (`terraform/environments/production/task-definition.json`):

```json
{
  "name": "DISCOVERY_S3_BUCKET",
  "value": "facilitator-discovery-prod"
},
{
  "name": "DISCOVERY_S3_KEY",
  "value": "bazaar/resources.json"
},
{
  "name": "FACILITATOR_URL",
  "value": "https://facilitator.ultravioletadao.xyz"
}
```

#### 2.4 Apply Terraform Changes

```bash
cd terraform/environments/production
terraform plan -out=discovery.tfplan
terraform apply discovery.tfplan
```

---

### Phase 3: Security Enhancements

**Objective**: Protect registration endpoint from abuse

#### 3.1 Rate Limiting (Recommended)

Add rate limiting middleware in `src/main.rs`:

```rust
use tower_governor::{GovernorConfigBuilder, GovernorLayer};

// Configure rate limiting
let governor_conf = GovernorConfigBuilder::default()
    .per_second(2)  // 2 registrations per second
    .burst_size(5)  // Allow bursts of 5
    .finish()
    .unwrap();

let discovery_routes = handlers::discovery_routes()
    .with_state(discovery_registry)
    .layer(GovernorLayer { config: governor_conf });
```

#### 3.2 API Key Authentication (Optional)

For a more secure registration, consider adding API key authentication:

**Option A: Header-based API Key**
```rust
// In handlers.rs
async fn post_discovery_register(
    State(registry): State<Arc<DiscoveryRegistry>>,
    headers: HeaderMap,
    Json(request): Json<RegisterResourceRequest>,
) -> impl IntoResponse {
    // Check API key
    let api_key = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    if !is_valid_api_key(api_key) {
        return (StatusCode::UNAUTHORIZED, Json(json!({
            "error": "Invalid or missing API key"
        }))).into_response();
    }
    // ... existing registration logic
}
```

**Option B: Signature-based Verification**
Require registrations to be signed by a wallet, proving ownership of the `payTo` address.

#### 3.3 Resource Validation

Add validation to prevent spam/abuse:

```rust
fn validate_resource(&self, resource: &DiscoveryResource) -> Result<(), DiscoveryError> {
    // Existing validations...

    // Add URL reachability check (optional, adds latency)
    // Add domain ownership verification
    // Add rate limiting per URL

    Ok(())
}
```

---

### Phase 4: Documentation and Promotion

**Objective**: Make the Discovery API discoverable to developers

#### 4.1 Update API Documentation

Add to `docs/API_REFERENCE.md`:

```markdown
## Discovery API (Bazaar)

The Discovery API allows x402-enabled services to register themselves for automatic discovery.

### List Resources

`GET /discovery/resources`

Query parameters:
- `limit` (default: 10, max: 100) - Number of resources to return
- `offset` (default: 0) - Pagination offset
- `category` - Filter by category (e.g., "finance", "ai")
- `network` - Filter by CAIP-2 network (e.g., "eip155:8453")
- `provider` - Filter by provider name
- `tag` - Filter by tag

Example:
```bash
curl "https://facilitator.ultravioletadao.xyz/discovery/resources?limit=10&category=finance"
```

### Register Resource

`POST /discovery/register`

Request body:
```json
{
  "url": "https://api.example.com/premium",
  "type": "http",
  "description": "Premium market data API",
  "accepts": [{
    "scheme": "exact",
    "network": "eip155:8453",
    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "amount": "10000",
    "payTo": "0x...",
    "maxTimeoutSeconds": 60
  }],
  "metadata": {
    "category": "finance",
    "provider": "Example Corp",
    "tags": ["market-data", "real-time"]
  }
}
```
```

#### 4.2 Update Landing Page

The landing page already documents the endpoints. Verify after deployment:
- `/discovery/resources` link works
- Bazaar section is visible

#### 4.3 Create Client Examples

Add example code for integrating with the Discovery API:

**Python Client**:
```python
import requests

# Discover available paid APIs
response = requests.get("https://facilitator.ultravioletadao.xyz/discovery/resources")
resources = response.json()["items"]

for resource in resources:
    print(f"Service: {resource['url']}")
    print(f"Price: {resource['accepts'][0]['amount']} USDC on {resource['accepts'][0]['network']}")
```

**JavaScript/TypeScript Client**:
```typescript
async function discoverServices() {
  const response = await fetch("https://facilitator.ultravioletadao.xyz/discovery/resources");
  const { items } = await response.json();
  return items;
}
```

---

## Verification Checklist

After implementation, verify:

- [ ] `GET /discovery/resources` returns 200 with empty items array
- [ ] `POST /discovery/register` successfully registers a test resource
- [ ] `GET /discovery/resources` returns the registered resource
- [ ] Pagination works (`?limit=1&offset=0`)
- [ ] Filtering works (`?category=test`)
- [ ] `GET /supported` includes `"extensions": ["bazaar"]`
- [ ] Self-registration works (facilitator appears in registry when FACILITATOR_URL is set)
- [ ] S3 persistence works (data survives container restart)
- [ ] Landing page Bazaar link works

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Spam registrations | Medium | Rate limiting, API key authentication |
| Malicious URLs registered | High | URL validation, manual review process |
| S3 costs | Low | JSON file is small, minimal operations |
| Discovery abuse | Medium | Read-only for public, write requires auth |

---

## Timeline Estimate

| Phase | Duration | Dependencies |
|-------|----------|--------------|
| Phase 1: Deploy | 1 hour | Docker build, ECS access |
| Phase 2: S3 Setup | 2-3 hours | Terraform, AWS permissions |
| Phase 3: Security | 4-6 hours | Code changes, testing |
| Phase 4: Documentation | 2-3 hours | None |

**Total**: 9-13 hours

---

## References

- [x402 V2 Specification](https://www.x402.org/writing/x402-v2-launch)
- [x402 GitBook Documentation](https://x402.gitbook.io/x402)
- [Coinbase x402 Repository](https://github.com/coinbase/x402)
- [CAIP-2 Chain Identifiers](https://chainagnostic.org/CAIPs/caip-2)
