# Blacklist Fix Deployment Guide

## Critical Security Issue: Blacklist Not Loading in Production

**Incident**: Malicious Solana wallet `41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az` drained facilitator funds despite being explicitly blacklisted.

**Root Cause**: The `config/blacklist.json` file was NOT being copied to the Docker runtime image, causing the application to silently fall back to an empty blacklist with zero protection.

---

## Changes Made

### 1. Dockerfile Fix (CRITICAL)

**File**: `Dockerfile` (lines 30-31)

**Change**: Added `COPY --from=builder /app/config /app/config` to ensure blacklist file exists at runtime.

```dockerfile
# Before (BROKEN):
COPY --from=builder /app/target/release/x402-rs /usr/local/bin/x402-rs

# After (FIXED):
COPY --from=builder /app/target/release/x402-rs /usr/local/bin/x402-rs
COPY --from=builder /app/config /app/config
```

**Why This Matters**: The multi-stage Docker build only copied the binary to the runtime stage. The `config/` directory was left behind in the builder stage, causing the file-not-found error.

---

### 2. New `/blacklist` Endpoint (Visibility)

**Added Endpoint**: `GET /blacklist`

**Purpose**: Provides runtime visibility into the current blacklist configuration being enforced. Essential for security auditing.

**Response Format**:
```json
{
  "total_blocked": 2,
  "evm_count": 1,
  "solana_count": 1,
  "entries": [
    {
      "account_type": "solana",
      "wallet": "41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az",
      "reason": "spam"
    },
    {
      "account_type": "evm",
      "wallet": "0x0000000000000000000000000000000000000000",
      "reason": "example blocked address"
    }
  ],
  "source": "config/blacklist.json",
  "loaded_at_startup": true
}
```

**Files Modified**:
- `src/handlers.rs` - Added `get_blacklist()` handler
- `src/main.rs` - Added route `/blacklist`
- `src/facilitator.rs` - Added `blacklist_info()` trait method
- `src/facilitator_local.rs` - Implemented `blacklist_info()` method
- `src/types.rs` - Added `BlacklistInfoResponse` and `BlacklistEntry` types
- `src/blocklist.rs` - Added `evm_count()`, `solana_count()`, `entries()` accessor methods

---

### 3. Enhanced Blacklist Loading (Security Hardening)

**File**: `src/main.rs` (lines 76-114)

**New Environment Variable**: `BLACKLIST_REQUIRED=true`

**Behavior**:
- When `BLACKLIST_REQUIRED=true`: Application **refuses to start** if blacklist cannot be loaded
- When `BLACKLIST_REQUIRED=false` (default): Application logs warning and starts with empty blacklist (backward compatible)

**Why This Matters**: In production, you want fail-fast behavior. If the blacklist file is missing or corrupt, the facilitator should NOT start with zero protection.

**New Startup Logs**:
```
[INFO] Successfully loaded blacklist: 1 EVM addresses, 1 Solana addresses, 2 total blocked
```

Or (if file missing):
```
[WARN] Failed to load config/blacklist.json: File read error: No such file or directory (os error 2). Using empty blacklist.
[WARN] Set BLACKLIST_REQUIRED=true to fail-fast if blacklist cannot be loaded.
```

Or (if BLACKLIST_REQUIRED=true and file missing):
```
[ERROR] BLACKLIST_REQUIRED=true but failed to load config/blacklist.json: File read error: No such file or directory
[ERROR] Refusing to start without blacklist protection. Exiting.
(process exits with code 1)
```

---

## Deployment Steps

### Step 1: Build New Docker Image

```bash
cd /path/to/facilitator

# Build with new version tag
docker build --platform linux/amd64 -t facilitator:v1.2.1-blacklist-fix .

# Tag for ECR
docker tag facilitator:v1.2.1-blacklist-fix <AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com/facilitator:v1.2.1-blacklist-fix
docker tag facilitator:v1.2.1-blacklist-fix <AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com/facilitator:latest

# Push to ECR
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin <AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com
docker push <AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com/facilitator:v1.2.1-blacklist-fix
docker push <AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com/facilitator:latest
```

**IMPORTANT**: Use the provided `scripts/build-and-push.sh` script for automated build/push:

```bash
./scripts/build-and-push.sh v1.2.1-blacklist-fix
```

---

### Step 2: Update ECS Task Definition

Option A: Use AWS Console
1. Go to ECS > Task Definitions > facilitator-production
2. Create new revision
3. Update container image to: `<AWS_ACCOUNT_ID>.dkr.ecr.us-east-1.amazonaws.com/facilitator:v1.2.1-blacklist-fix`
4. Add environment variable: `BLACKLIST_REQUIRED=true` (RECOMMENDED for production security)
5. Save new revision

Option B: Use AWS CLI
```bash
# Get current task definition
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-1 > current-task-def.json

# Edit current-task-def.json:
# 1. Update image to v1.2.1-blacklist-fix
# 2. Add environment variable: {"name": "BLACKLIST_REQUIRED", "value": "true"}
# 3. Remove fields: taskDefinitionArn, revision, status, requiresAttributes, compatibilities, registeredAt, registeredBy

# Register new revision
aws ecs register-task-definition \
  --cli-input-json file://current-task-def.json \
  --region us-east-1
```

---

### Step 3: Deploy to ECS

```bash
# Force new deployment with updated task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-1

# Monitor deployment
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-1 \
  --query 'services[0].deployments'
```

---

### Step 4: Validate Blacklist is Working

#### 4.1 Check Startup Logs in CloudWatch

```bash
aws logs tail /ecs/facilitator-production --follow --region us-east-1
```

**Expected Output** (SUCCESS):
```
[INFO] Successfully loaded blacklist: 1 EVM addresses, 1 Solana addresses, 2 total blocked
```

**Failure Indicator**:
```
[WARN] Failed to load config/blacklist.json: File read error: No such file or directory
```

If you see the failure message, the Dockerfile fix was not applied correctly. STOP and investigate.

#### 4.2 Test `/blacklist` Endpoint

```bash
# Production URL
curl https://facilitator.prod.ultravioletadao.xyz/blacklist | jq

# Expected response:
{
  "totalBlocked": 2,
  "evmCount": 1,
  "solanaCount": 1,
  "entries": [
    {
      "account_type": "solana",
      "wallet": "41fx2qju8qceppdlwnypgxahagj3dfvi8bhfumteq3az",
      "reason": "spam"
    },
    {
      "account_type": "evm",
      "wallet": "0x0000000000000000000000000000000000000000",
      "reason": "example blocked address"
    }
  ],
  "source": "config/blacklist.json",
  "loadedAtStartup": true
}
```

**Critical Check**: Verify `loadedAtStartup: true` and `totalBlocked > 0`

#### 4.3 Test Blacklist Enforcement

```bash
# Try to verify a payment from the blacklisted Solana wallet
# (This should FAIL with a BlockedAddress error)

curl -X POST https://facilitator.prod.ultravioletadao.xyz/verify \
  -H "Content-Type: application/json" \
  -d '{
    "x402Version": 1,
    "paymentPayload": {
      "scheme": "exact",
      "network": "solana-devnet",
      "payload": {
        "transaction": "..."
      }
    },
    "paymentRequirements": {
      "payTo": "...",
      "amount": {
        "asset": {
          "kind": "token",
          "symbol": "USDC",
          "decimals": 6
        },
        "value": "1000000"
      }
    }
  }'
```

**Expected**: Request should fail with error mentioning "BlockedAddress" or "Blocked sender"

**If the request succeeds**: Blacklist enforcement is NOT working. Investigate immediately.

---

## Rollback Plan

If deployment fails:

```bash
# Rollback to previous task definition revision
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:<PREVIOUS_REVISION> \
  --force-new-deployment \
  --region us-east-1
```

To find previous revision:
```bash
aws ecs list-task-definitions \
  --family-prefix facilitator-production \
  --region us-east-1 \
  --sort DESC \
  --max-items 5
```

---

## Post-Deployment Monitoring

### CloudWatch Logs to Monitor

1. **Successful blacklist loading**:
   - Filter: `Successfully loaded blacklist`
   - Expected: Should appear on every container startup

2. **Blocked payment attempts**:
   - Filter: `Blocked EVM address` OR `Blocked Solana address`
   - Expected: Should log whenever a blacklisted wallet tries to pay

3. **Blacklist endpoint access**:
   - Filter: `GET /blacklist`
   - Use for audit trail of who checked blacklist configuration

### CloudWatch Alarms to Create

1. **Empty Blacklist Alert**:
   - Metric: Log pattern `Using empty blacklist`
   - Threshold: >= 1 occurrence
   - Action: Send SNS notification
   - **This indicates blacklist failed to load**

2. **Blacklist Bypass Attempt**:
   - Metric: Log pattern `Blocked.*attempted payment`
   - Threshold: >= 5 occurrences in 5 minutes
   - Action: Send SNS notification
   - **This indicates an attack in progress**

---

## Security Recommendations

### 1. Enable Fail-Fast Mode in Production

Add `BLACKLIST_REQUIRED=true` to ECS task definition environment variables.

**Why**: If blacklist file is corrupted or missing, facilitator will refuse to start rather than silently accepting all payments.

**Trade-off**: Slightly less resilient to config errors, but much safer.

### 2. Monitor `/blacklist` Endpoint Daily

Create a cron job or CloudWatch scheduled event to:
1. Fetch `/blacklist` endpoint
2. Verify `totalBlocked > 0`
3. Alert if `loadedAtStartup: false`

```bash
#!/bin/bash
# Daily blacklist health check

RESPONSE=$(curl -s https://facilitator.prod.ultravioletadao.xyz/blacklist)
TOTAL_BLOCKED=$(echo "$RESPONSE" | jq -r '.totalBlocked')
LOADED=$(echo "$RESPONSE" | jq -r '.loadedAtStartup')

if [ "$TOTAL_BLOCKED" -eq 0 ] || [ "$LOADED" = "false" ]; then
  echo "CRITICAL: Blacklist not loaded properly!"
  # Send alert via SNS or email
  exit 1
fi

echo "Blacklist health check passed: $TOTAL_BLOCKED addresses blocked"
```

### 3. Version Control for Blacklist

Add blacklist updates to your deployment pipeline:
1. Update `config/blacklist.json` in Git
2. Commit with message: "chore: add wallet XXX to blacklist (reason: YYY)"
3. Create new Docker image
4. Deploy via ECS service update
5. Verify with `/blacklist` endpoint

### 4. Audit Trail

All blacklist checks are logged at WARN level:
```
[WARN] Blocked Solana address (Blocked sender) attempted payment: 41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az - Reason: Blocked sender: spam
```

Use CloudWatch Insights to track blocked attempts:
```
fields @timestamp, @message
| filter @message like /Blocked.*attempted payment/
| sort @timestamp desc
| limit 100
```

---

## Files Modified Summary

| File | Change | Purpose |
|------|--------|---------|
| `Dockerfile` | Added `COPY --from=builder /app/config /app/config` | Fix: Include blacklist file in runtime image |
| `src/handlers.rs` | Added `get_blacklist()` handler | New `/blacklist` endpoint |
| `src/main.rs` | Added `/blacklist` route, enhanced loading | Expose endpoint, fail-fast option |
| `src/facilitator.rs` | Added `blacklist_info()` trait method | API contract |
| `src/facilitator_local.rs` | Implemented `blacklist_info()` | Return blacklist data |
| `src/types.rs` | Added `BlacklistInfoResponse`, `BlacklistEntry` | API response types |
| `src/blocklist.rs` | Added accessor methods | Expose internal data |

---

## Testing Checklist

- [ ] Docker image builds successfully
- [ ] `config/blacklist.json` exists in final Docker image (verify with `docker run --entrypoint ls facilitator:v1.2.1-blacklist-fix -la /app/config`)
- [ ] Application starts and logs "Successfully loaded blacklist"
- [ ] `/blacklist` endpoint returns correct data
- [ ] Blacklisted Solana wallet is rejected on `/verify`
- [ ] Blacklisted EVM wallet is rejected on `/verify`
- [ ] Non-blacklisted wallets still work correctly
- [ ] CloudWatch logs show blacklist blocking events
- [ ] `BLACKLIST_REQUIRED=true` causes startup failure if file missing (test locally)

---

## Additional Security Measures

### Dynamic Blacklist Updates (Future Enhancement)

Current implementation requires Docker rebuild to update blacklist. Consider:

1. **AWS Secrets Manager Integration**:
   - Store blacklist as JSON in Secrets Manager
   - Load at startup from Secrets Manager
   - Enable hot-reload via API endpoint (requires authentication)

2. **Database-Backed Blacklist**:
   - Store in DynamoDB or RDS
   - Admin UI for adding/removing addresses
   - Audit log of all blacklist changes

3. **Circuit Breaker Pattern**:
   - If blacklist fails to load, reject ALL payments until fixed
   - Prevents silent failures

---

## Questions?

Contact the infrastructure team or refer to:
- `BLACKLIST_IMPLEMENTATION.md` - Original blacklist implementation docs
- `BLACKLIST_REFACTORING_SUMMARY.md` - Blacklist refactoring history
- `docs/WALLET_ROTATION.md` - Security procedures
