# Blacklist Quick Reference Card

## TL;DR - What Was Fixed

**Problem**: Blacklist file wasn't being loaded in production (Docker image issue)
**Impact**: Malicious wallet drained facilitator despite being blacklisted
**Fix**: Updated Dockerfile + added `/blacklist` endpoint + fail-fast option

---

## Quick Commands

### Check if Blacklist is Working (Production)
```bash
curl https://facilitator.prod.ultravioletadao.xyz/blacklist | jq
```

**Good Response**:
```json
{
  "totalBlocked": 2,        ← Should be > 0
  "loadedAtStartup": true,  ← MUST be true
  "entries": [...]
}
```

**Bad Response**:
```json
{
  "totalBlocked": 0,         ← ALERT!
  "loadedAtStartup": false,  ← ALERT!
  "entries": []
}
```

### Check CloudWatch Logs
```bash
aws logs tail /ecs/facilitator-production --follow --region us-east-1 | grep -i blacklist
```

**Good Log**:
```
[INFO] Successfully loaded blacklist: 1 EVM addresses, 1 Solana addresses, 2 total blocked
```

**Bad Log**:
```
[WARN] Failed to load config/blacklist.json: ... Using empty blacklist.
```

### Verify Malicious Wallet is Blocked
```bash
curl -s https://facilitator.prod.ultravioletadao.xyz/blacklist | \
  jq '.entries[] | select(.wallet | contains("41fx2qju8q"))'
```

**Expected**:
```json
{
  "account_type": "solana",
  "wallet": "41fx2qju8qceppdlwnypgxahagj3dfvi8bhfumteq3az",
  "reason": "spam"
}
```

---

## Deployment (One-Liner)

```bash
# Build, push, and deploy
./scripts/build-and-push.sh v1.2.1-blacklist-fix && \
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-1
```

Then run validation:
```bash
./scripts/validate_blacklist_fix.sh prod
```

---

## Files Changed

| File | What Changed |
|------|--------------|
| `Dockerfile` | Added `COPY --from=builder /app/config /app/config` |
| `src/handlers.rs` | New `get_blacklist()` endpoint |
| `src/main.rs` | Added `/blacklist` route + fail-fast logic |
| `src/facilitator.rs` | Added `blacklist_info()` trait method |
| `src/facilitator_local.rs` | Implemented `blacklist_info()` |
| `src/types.rs` | Added `BlacklistInfoResponse` type |
| `src/blocklist.rs` | Added accessor methods |

---

## New Environment Variable

**Variable**: `BLACKLIST_REQUIRED`
**Default**: `false`
**Recommended Production**: `true`

**What it does**:
- `true` → Refuses to start if blacklist fails to load (SAFE)
- `false` → Logs warning and starts with empty blacklist (UNSAFE)

**How to set** (in ECS task definition):
```json
{
  "name": "BLACKLIST_REQUIRED",
  "value": "true"
}
```

---

## Troubleshooting

### Issue: `/blacklist` endpoint returns 404

**Cause**: Old version deployed
**Fix**: Rebuild and redeploy with new code

```bash
./scripts/build-and-push.sh v1.2.1-blacklist-fix
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-1
```

---

### Issue: `totalBlocked: 0` in response

**Cause**: Blacklist file missing from Docker image
**Fix**: Verify Dockerfile has `COPY --from=builder /app/config /app/config`

**Verify**:
```bash
docker run --rm --entrypoint ls facilitator:latest -la /app/config/blacklist.json
```

**Expected**: File listing (not "No such file")

---

### Issue: `loadedAtStartup: false`

**Cause**: Blacklist file failed to load at startup
**Diagnosis**: Check CloudWatch logs for error

```bash
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "Failed to load" \
  --region us-east-1 \
  --max-items 10
```

**Common causes**:
1. File not in Docker image → Check Dockerfile
2. File permissions wrong → Should be readable by app user
3. File corrupted → Validate JSON syntax in `config/blacklist.json`

---

### Issue: Container won't start (with BLACKLIST_REQUIRED=true)

**Cause**: Blacklist file genuinely missing - this is **INTENTIONAL**
**Action**: Fix the root cause (add blacklist file to image)

This is **SAFE BEHAVIOR** - the container should NOT start without blacklist protection.

---

## Adding a Wallet to Blacklist

1. **Edit** `config/blacklist.json`:
   ```json
   [
     {
       "account_type": "solana",
       "wallet": "MALICIOUS_WALLET_ADDRESS",
       "reason": "spam / scam / exploit"
     }
   ]
   ```

2. **Commit** to Git:
   ```bash
   git add config/blacklist.json
   git commit -m "chore: blacklist wallet MALICIOUS_WALLET_ADDRESS (reason: spam)"
   ```

3. **Build and deploy**:
   ```bash
   ./scripts/build-and-push.sh v1.2.1-blacklist-fix
   aws ecs update-service --cluster facilitator-production \
     --service facilitator-production --force-new-deployment --region us-east-1
   ```

4. **Verify**:
   ```bash
   curl -s https://facilitator.prod.ultravioletadao.xyz/blacklist | \
     jq '.entries[] | select(.wallet == "MALICIOUS_WALLET_ADDRESS")'
   ```

**Note**: This requires rebuilding the Docker image. For hot-reload capability, see future enhancement in `BLACKLIST_FIX_SUMMARY.md`.

---

## Monitoring Alerts to Create

### Alert 1: Empty Blacklist
- **Metric**: CloudWatch Logs pattern match
- **Pattern**: `"Using empty blacklist"`
- **Threshold**: >= 1 occurrence
- **Action**: Page on-call engineer
- **Severity**: CRITICAL

### Alert 2: Blacklist Load Failure
- **Metric**: CloudWatch Logs pattern match
- **Pattern**: `"Failed to load config/blacklist.json"`
- **Threshold**: >= 1 occurrence
- **Action**: Email ops team
- **Severity**: HIGH

### Alert 3: Blocked Wallet Attempts
- **Metric**: CloudWatch Logs pattern match
- **Pattern**: `"Blocked.*attempted payment"`
- **Threshold**: >= 10 occurrences in 5 minutes
- **Action**: Email security team
- **Severity**: MEDIUM (informational, blacklist is working)

---

## Testing in Staging/Dev

```bash
# Build image
docker build -t facilitator:test .

# Verify config file exists
docker run --rm --entrypoint ls facilitator:test -la /app/config/blacklist.json

# Run locally
docker run -p 8080:8080 --env-file .env facilitator:test

# Test endpoint
curl http://localhost:8080/blacklist | jq

# Test fail-fast mode (should exit immediately)
docker run --rm -e BLACKLIST_REQUIRED=true \
  --entrypoint sh facilitator:test \
  -c "rm /app/config/blacklist.json && x402-rs"
```

---

## Rollback Procedure

If new deployment breaks:

```bash
# List recent task definitions
aws ecs list-task-definitions \
  --family-prefix facilitator-production \
  --region us-east-1 \
  --sort DESC \
  --max-items 5

# Rollback to previous revision
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:<REVISION_BEFORE_DEPLOY> \
  --force-new-deployment \
  --region us-east-1

# Verify rollback
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-1 \
  --query 'services[0].deployments'
```

---

## Health Check (Daily)

Run this daily to verify blacklist is working:

```bash
#!/bin/bash
TOTAL=$(curl -s https://facilitator.prod.ultravioletadao.xyz/blacklist | jq -r '.totalBlocked')
LOADED=$(curl -s https://facilitator.prod.ultravioletadao.xyz/blacklist | jq -r '.loadedAtStartup')

if [ "$TOTAL" -eq 0 ] || [ "$LOADED" = "false" ]; then
  echo "ALERT: Blacklist not working! totalBlocked=$TOTAL loadedAtStartup=$LOADED"
  exit 1
else
  echo "OK: Blacklist active with $TOTAL blocked addresses"
fi
```

---

## Useful Links

- **Deployment Guide**: `BLACKLIST_FIX_DEPLOYMENT.md`
- **Technical Summary**: `BLACKLIST_FIX_SUMMARY.md`
- **Validation Script**: `scripts/validate_blacklist_fix.sh`
- **Production URL**: https://facilitator.prod.ultravioletadao.xyz
- **CloudWatch Logs**: `/ecs/facilitator-production` (us-east-1)
- **ECS Cluster**: `facilitator-production` (us-east-1)
- **ECR Repository**: `facilitator` (us-east-1)

---

## Emergency Contact

**Security Issue**: Escalate to security team immediately
**Production Down**: Page on-call SRE
**Blacklist Bypass**: Add to blacklist within 5 minutes (see "Adding a Wallet to Blacklist" above)
