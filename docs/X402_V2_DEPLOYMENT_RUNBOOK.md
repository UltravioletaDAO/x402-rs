# x402 v2 Deployment Runbook

**Quick Reference for Infrastructure Team**

**Date:** 2025-12-11
**Author:** Infrastructure Team
**Target Environment:** Production (facilitator.ultravioletadao.xyz)

---

## Pre-Deployment Checklist

- [ ] Read `X402_V2_INFRASTRUCTURE_ANALYSIS.md` (full context)
- [ ] Read `X402_V2_ANALYSIS.md` (protocol changes)
- [ ] Confirm Rust application has v2 support implemented
- [ ] Backup current ECS task definition revision
- [ ] Note current CloudWatch baseline metrics
- [ ] Verify rollback procedure

---

## Step-by-Step Deployment

### Step 1: Apply Terraform Changes (CloudWatch Monitoring)

**Estimated Time:** 5 minutes

```bash
# Navigate to production terraform
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Review changes
terraform plan -out=v2-migration.tfplan

# Expected changes:
# + 7 CloudWatch metric filters
# + 3 CloudWatch alarms
# + 1 CloudWatch dashboard
# Total: 11 new resources, ~$4/month cost increase

# Apply changes
terraform apply v2-migration.tfplan

# Save dashboard URL (displayed in output)
# Example: https://console.aws.amazon.com/cloudwatch/home?region=us-east-2#dashboards:name=facilitator-x402-v2-migration
```

**Verification:**
```bash
# Verify dashboard exists
aws cloudwatch get-dashboard \
  --dashboard-name facilitator-x402-v2-migration \
  --region us-east-2

# Verify metric filters created
aws logs describe-metric-filters \
  --log-group-name /ecs/facilitator-production \
  --region us-east-2 | grep x402
```

**Timestamp:** `date -u`

---

### Step 2: Build and Push Docker Image (Dual v1+v2 Support)

**Estimated Time:** 10 minutes

**Prerequisites:**
- Rust application has v2 support merged to `pr-2-fogo-updated` branch
- Application reads `X402_VERSION_SUPPORT` environment variable
- CAIP-2 parsing implemented in `src/network.rs`

```bash
# Navigate to project root
cd Z:\ultravioleta\dao\x402-rs

# Verify application code has v2 support
grep -r "X402_VERSION_SUPPORT" src/
grep -r "caip2" src/network.rs

# Build Docker image
docker build -t facilitator:v2.0.0-dual .

# Test locally (optional but recommended)
docker run -p 8080:8080 \
  -e X402_VERSION_SUPPORT=v1,v2 \
  -e RUST_LOG=debug \
  facilitator:v2.0.0-dual

# In another terminal, test endpoints
curl http://localhost:8080/health
curl http://localhost:8080/supported

# Push to ECR
./scripts/build-and-push.sh v2.0.0-dual
```

**Verification:**
```bash
# Verify image in ECR
aws ecr describe-images \
  --repository-name facilitator \
  --region us-east-2 \
  --query 'imageDetails[?imageTags[?contains(@, `v2.0.0-dual`)]]'
```

**Timestamp:** `date -u`

---

### Step 3: Update ECS Task Definition

**Estimated Time:** 3 minutes

**Option A: Via Terraform (Recommended)**

Edit `terraform/environments/production/variables.tf`:

```hcl
variable "image_tag" {
  description = "Docker image tag"
  type        = string
  default     = "v2.0.0-dual"  # Changed from "v1.3.6"
}

variable "x402_version_support" {
  description = "Supported x402 protocol versions"
  type        = string
  default     = "v1,v2"  # NEW
}
```

Edit `terraform/environments/production/main.tf` (line ~479):

```hcl
environment = [
  {
    name  = "RUST_LOG"
    value = "info"
  },
  {
    name  = "X402_VERSION_SUPPORT"  # NEW
    value = var.x402_version_support
  },
  # ... rest of existing variables
]
```

Apply changes:

```bash
cd terraform/environments/production
terraform plan -out=v2-task-def.tfplan
terraform apply v2-task-def.tfplan
```

**Option B: Via AWS CLI (Faster for testing)**

```bash
# Get current task definition
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 \
  --query 'taskDefinition' > /tmp/current-task-def.json

# Edit JSON (add X402_VERSION_SUPPORT environment variable)
# Manually edit /tmp/current-task-def.json, add to containerDefinitions[0].environment:
# {
#   "name": "X402_VERSION_SUPPORT",
#   "value": "v1,v2"
# }

# Register new task definition
aws ecs register-task-definition \
  --cli-input-json file:///tmp/current-task-def.json \
  --region us-east-2

# Output: Revision number (e.g., 42)
```

**Timestamp:** `date -u`

---

### Step 4: Deploy to ECS (Rolling Update)

**Estimated Time:** 10 minutes (rolling deployment)

```bash
# Option 1: Update service to use new task definition (from Step 3)
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:42 \
  --force-new-deployment \
  --region us-east-2

# Option 2: Force redeployment with same task definition (if using Terraform)
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

**Monitor Deployment:**

```bash
# Watch deployment progress
watch -n 5 'aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query "services[0].deployments" \
  --output table'

# Expected output:
# PRIMARY deployment: desiredCount=1, runningCount=1, pendingCount=0 (NEW)
# ACTIVE deployment: desiredCount=0, runningCount=0, pendingCount=0 (OLD, draining)
```

**Wait for:**
- NEW deployment reaches `runningCount=1`
- OLD deployment reaches `runningCount=0`
- Health checks pass (2/2 healthy in target group)

**Timestamp:** `date -u`

---

### Step 5: Verify Deployment

**Estimated Time:** 5 minutes

#### Health Check
```bash
curl https://facilitator.ultravioletadao.xyz/health
# Expected: {"status":"healthy"}
```

#### Check ECS Logs
```bash
aws logs tail /ecs/facilitator-production --follow --region us-east-2

# Look for:
# - "X402_VERSION_SUPPORT=v1,v2" in startup logs
# - No errors related to CAIP-2 or version parsing
```

#### Test v1 Endpoint (Backward Compatibility)
```bash
curl -X GET https://facilitator.ultravioletadao.xyz/supported | jq

# Should include v1 networks like "base-sepolia", "avalanche-fuji"
```

#### Test v2 Endpoint (New Functionality)
```bash
# This requires actual v2 payload - wait for Rust team to provide test payload
# Placeholder test: Check /supported returns both v1 and v2 formats
```

#### Verify ALB Target Health
```bash
aws elbv2 describe-target-health \
  --target-group-arn $(aws elbv2 describe-target-groups \
    --names facilitator-production \
    --region us-east-2 \
    --query 'TargetGroups[0].TargetGroupArn' \
    --output text) \
  --region us-east-2

# Expected: All targets "healthy"
```

#### Check CloudWatch Dashboard
```bash
# Open dashboard URL (from Step 1 output)
# Verify:
# - "x402 Protocol Version Adoption" widget shows data
# - "CAIP-2 Parsing Errors" is zero
# - Recent log events appear
```

**Timestamp:** `date -u`

---

### Step 6: Monitor for 24 Hours

**Active Monitoring (First 2 Hours):**
- [ ] Check CloudWatch dashboard every 15 minutes
- [ ] Monitor ECS service events for errors
- [ ] Watch CloudWatch Logs for CAIP-2 parsing errors
- [ ] Verify no increase in 5xx error rate on ALB

**Passive Monitoring (Next 22 Hours):**
- [ ] Set up CloudWatch alarm notifications (if not already)
- [ ] Check dashboard once every 4 hours
- [ ] Review error logs at end of 24 hours

**CloudWatch Logs Insights Queries:**

```bash
# Query 1: Check v1 vs v2 traffic distribution
# fields @timestamp, x402_version
# | stats count() by x402_version

# Query 2: Any CAIP-2 errors?
# fields @timestamp, @message
# | filter @message like /CAIP-2/
# | filter level = "ERROR"

# Query 3: v2 settlement success rate
# fields @timestamp, @message
# | filter x402_version = 2
# | filter @message like /settlement/
# | stats count(*) as total,
#         sum(level = "INFO") as success,
#         sum(level = "ERROR") as failure
```

**Timestamp:** `date -u`

---

## Rollback Procedure

**If deployment fails or causes issues:**

### Immediate Rollback (< 5 minutes)

```bash
# Step 1: Get previous task definition revision
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 \
  --query 'taskDefinition.revision'

# Example output: Current revision is 42, previous is 41

# Step 2: Rollback ECS service
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:41 \
  --force-new-deployment \
  --region us-east-2

# Step 3: Monitor rollback
watch -n 5 'aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query "services[0].deployments"'

# Step 4: Verify health
curl https://facilitator.ultravioletadao.xyz/health
```

### Terraform Rollback (if infrastructure was changed)

```bash
cd terraform/environments/production

# Revert Terraform files
git checkout HEAD~1 -- main.tf variables.tf

# Apply previous configuration
terraform apply -auto-approve

# Force ECS service update
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

**Timestamp:** `date -u`

---

## Post-Deployment Tasks

### Week 1
- [ ] Daily check of CloudWatch dashboard
- [ ] Document any v2 client feedback
- [ ] Adjust alarm thresholds if needed

### Month 1
- [ ] Review v1 vs v2 traffic ratio
- [ ] Publish blog post announcing v2 support
- [ ] Update documentation with v2 examples

### Month 6
- [ ] Verify v2 traffic > 95% of total
- [ ] Plan v1 deprecation announcement
- [ ] Prepare v2-only task definition

---

## Key Metrics to Track

| Metric | Target | Alert Threshold |
|--------|--------|-----------------|
| v2 Adoption % | > 10% after Week 1 | < 5% after Week 2 |
| CAIP-2 Parsing Errors | 0 | > 5 in 5 minutes |
| v2 Settlement Success Rate | > 95% | < 90% |
| ECS Task Health | 100% healthy | < 100% for > 5 minutes |
| ALB 5xx Error Rate | < 0.1% | > 1% |
| Response Time (p99) | < 500ms | > 1000ms |

---

## Troubleshooting

### Issue: CAIP-2 Parsing Errors

**Symptoms:** CloudWatch alarm `facilitator-caip2-parsing-errors-high` triggered

**Diagnosis:**
```bash
# Check logs for specific error messages
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "CAIP-2" \
  --region us-east-2 \
  --limit 20

# Look for malformed network identifiers
```

**Resolution:**
1. Contact Rust team to review `src/network.rs` CAIP-2 parsing logic
2. If critical, rollback to v1-only
3. Fix parsing bug and redeploy

---

### Issue: High v2 Settlement Failure Rate

**Symptoms:** CloudWatch alarm `facilitator-v2-settlement-failure-rate-high` triggered

**Diagnosis:**
```bash
# Check settlement error messages
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "x402_version=2 settlement" \
  --region us-east-2

# Check RPC connectivity
curl -X POST https://facilitator.ultravioletadao.xyz/settle \
  -H "Content-Type: application/json" \
  -d '{"x402Version":2, ...}' -v
```

**Resolution:**
1. Verify RPC endpoints in Secrets Manager
2. Check facilitator wallet has gas funds
3. Review on-chain transaction failures
4. If widespread, rollback and investigate

---

### Issue: v1 Traffic Sudden Drop

**Symptoms:** CloudWatch alarm `facilitator-x402-v1-traffic-sudden-drop` triggered

**Diagnosis:**
- Check if clients migrated to v2 unexpectedly
- Verify v1 endpoint still functional
- Review ECS logs for v1 request handling

**Resolution:**
1. Test v1 endpoint manually
2. If v1 broken, rollback immediately
3. If clients migrated early, monitor closely

---

## Contact Information

**Infrastructure Team:** infra@ultravioletadao.xyz
**Rust Team:** dev@ultravioletadao.xyz
**On-Call:** Slack #facilitator-alerts

**Escalation:**
1. Check runbook
2. Review CloudWatch logs
3. Ping #facilitator-alerts
4. If critical, rollback first, debug later

---

## Appendix: Environment Variables Reference

| Variable | Value | Purpose |
|----------|-------|---------|
| `X402_VERSION_SUPPORT` | `v1,v2` | Enable dual protocol support |
| `RUST_LOG` | `info` | Logging level |
| `PORT` | `8080` | Container port |
| `HOST` | `0.0.0.0` | Bind address |
| `SIGNER_TYPE` | `private-key` | Wallet type |

**After v1 deprecation (Month 6+):**
```bash
X402_VERSION_SUPPORT=v2  # Remove v1 support
```

---

## Success Criteria

**Deployment is successful if:**
- [x] ECS service reports 100% healthy targets
- [x] `/health` endpoint returns 200 OK
- [x] CloudWatch dashboard shows v1 and v2 traffic
- [x] No CAIP-2 parsing errors in first hour
- [x] v1 backward compatibility maintained
- [x] No increase in error rate or latency

**Migration is successful if (after 6 months):**
- [x] v2 traffic > 95% of total
- [x] CAIP-2 parsing errors < 1/week
- [x] Zero client complaints
- [x] Response time unchanged
- [x] Ready to deprecate v1

---

**End of Runbook**

**Last Updated:** 2025-12-11
**Next Review:** After deployment completion
