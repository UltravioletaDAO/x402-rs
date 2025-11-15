# Production Deployment Commands for v1.3.11

**Version**: v1.3.11
**Features**: BSC mainnet + testnet with BUSD token support
**Date**: 2025-11-15

---

## Prerequisites

- Docker Desktop running
- AWS CLI configured with credentials
- In directory: `Z:\ultravioleta\dao\x402-rs`

---

## Phase 1: Build Docker Image

```bash
# Navigate to project
cd /mnt/z/ultravioleta/dao/x402-rs

# Build with version tag (CRITICAL: include --build-arg)
docker build --platform linux/amd64 --build-arg FACILITATOR_VERSION=v1.3.11 -t facilitator:v1.3.11 .
```

Expected output: Build succeeds with ~30 layers

---

## Phase 2: Push to ECR

```bash
# Tag for ECR
docker tag facilitator:v1.3.11 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.3.11

# Login to ECR
aws ecr get-login-password --region us-east-2 | docker login --username AWS --password-stdin 518898403364.dkr.ecr.us-east-2.amazonaws.com

# Push to ECR
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.3.11

# Also tag and push as latest
docker tag facilitator:v1.3.11 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:latest
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:latest
```

Expected output: Push completes, layers uploaded to ECR

---

## Phase 3: Update Task Definition

```bash
# Get current task definition
aws ecs describe-task-definition --task-definition facilitator-production --region us-east-2 --query 'taskDefinition' > task-def-base.json

# Clean task definition (remove AWS metadata)
cat task-def-base.json | jq 'del(.taskDefinitionArn, .revision, .status, .requiresAttributes, .placementConstraints, .compatibilities, .registeredAt, .registeredBy)' > task-def-clean.json

# Update image tag
cat task-def-clean.json | jq '.containerDefinitions[0].image = "518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.3.11"' > task-def-updated.json

# Register new task definition
aws ecs register-task-definition --cli-input-json file://task-def-updated.json --region us-east-2 --query 'taskDefinition.{family:family,revision:revision}'
```

Expected output: New revision number (e.g., revision 14 or 15)

Note the revision number - you'll need it for the next step!

---

## Phase 4: Deploy to ECS

```bash
# Update service with new task definition (replace [REVISION] with actual number)
aws ecs update-service --cluster facilitator-production --service facilitator-production --task-definition facilitator-production:[REVISION] --force-new-deployment --region us-east-2

# Example if revision is 14:
# aws ecs update-service --cluster facilitator-production --service facilitator-production --task-definition facilitator-production:14 --force-new-deployment --region us-east-2
```

Expected output: Service update initiated

---

## Phase 5: Monitor Deployment

```bash
# Wait 60 seconds for deployment to start
sleep 60

# Check deployment status
aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query 'services[0].deployments[*].{status:status,running:runningCount,rolloutState:rolloutState}'
```

Expected output:
- `rolloutState: "IN_PROGRESS"` initially
- `rolloutState: "COMPLETED"` after 2-3 minutes

---

## Phase 6: Verify Deployment

```bash
# Check version endpoint (CRITICAL - must show v1.3.11)
curl https://facilitator.ultravioletadao.xyz/version

# Expected: {"version":"v1.3.11"}

# Check health
curl https://facilitator.ultravioletadao.xyz/health | jq

# Expected: {"status":"healthy"}

# Verify BSC networks are present
curl https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | contains("bsc"))'

# Expected output should show:
# - bsc-mainnet with usdc/eip3009 scheme
# - bsc-testnet with usdc/eip3009 scheme
```

---

## Phase 7: Check Logs (Optional)

```bash
# List running tasks
aws ecs list-tasks --cluster facilitator-production --service-name facilitator-production --desired-status RUNNING --region us-east-2

# Tail logs
aws logs tail /ecs/facilitator-production --follow --region us-east-2 | grep -E "(Successfully loaded blacklist|Starting server|BSC)"
```

---

## Cleanup

```bash
# Remove temporary files
rm -f task-def-base.json task-def-clean.json task-def-updated.json
```

---

## Success Criteria

- ✅ Version endpoint returns `{"version":"v1.3.11"}`
- ✅ Health endpoint returns `{"status":"healthy"}`
- ✅ `/supported` endpoint includes `bsc-mainnet` and `bsc-testnet`
- ✅ Landing page loads at https://facilitator.ultravioletadao.xyz
- ✅ ECS service shows `rolloutState: "COMPLETED"`
- ✅ No errors in CloudWatch logs

---

## Rollback (if needed)

```bash
# List task definition revisions
aws ecs list-task-definitions --family-prefix facilitator-production --region us-east-2

# Rollback to previous revision (e.g., revision 13)
aws ecs update-service --cluster facilitator-production --service facilitator-production --task-definition facilitator-production:13 --force-new-deployment --region us-east-2
```

---

## Troubleshooting

**Issue**: Version endpoint doesn't show v1.3.11
**Fix**: Verify `--build-arg FACILITATOR_VERSION=v1.3.11` was included in docker build

**Issue**: BSC networks not in `/supported`
**Fix**: Check that v1.3.11 code is properly committed with BSC additions in src/network.rs

**Issue**: Task fails health checks
**Fix**: Check CloudWatch logs for startup errors

**Issue**: "Invalid signature" errors
**Fix**: Verify AWS Secrets Manager has correct wallet keys for mainnet/testnet

---

## Notes

- Total deployment time: ~8-12 minutes
- Old tasks drain gracefully (2-5 minutes)
- ECS performs rolling update (zero downtime)
- Can run `/test-prod` slash command after deployment to verify all endpoints

---

**Deployment prepared for v1.3.11**
BSC Mainnet + Testnet with BUSD Token Support
