# Rollback Plan for v1.2.0 Deployment

**Date**: 2025-11-06
**Current Version**: v1.1.1 (Task Definition: facilitator-production:14)
**Target Version**: v1.2.0 (Upstream v0.9.1 + All customizations)
**Deployment Time**: TBD

## Current Production State

- **Service**: facilitator-production
- **Cluster**: facilitator-production
- **Region**: us-east-2
- **Task Definition**: facilitator-production:14
- **Running Tasks**: 1/1 (healthy)
- **Current Image**: 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.1.1
- **Health Check**: Passing (ALB /health endpoint)

## What's Changing in v1.2.0

### Upstream Security Fixes (v0.9.1)
1. **Fee payer safety checks** in Solana settlement (critical security fix)
   - Prevents self-payment attacks
   - Stricter ATA creation validation
   - Compute unit limit parsing

2. **Version bump** to 0.9.1

### Our Customizations (PRESERVED)
- ✅ Ultravioleta DAO branding (landing page, logos)
- ✅ 6 custom networks (HyperEVM, Polygon, Optimism, Celo)
- ✅ Blacklist implementation
- ✅ All infrastructure code
- ✅ Rust edition 2021 (downgraded for compatibility)

## Pre-Deployment Checklist

- [ ] Build v1.2.0 Docker image locally: `cargo build --release`
- [ ] Push v1.2.0 to ECR: `./scripts/build-and-push.sh v1.2.0`
- [ ] Verify image in ECR: `aws ecr describe-images --repository-name facilitator --region us-east-2`
- [ ] Create new ECS task definition with v1.2.0 image
- [ ] Document rollback commands (below)
- [ ] Monitor health endpoint during deployment
- [ ] Test critical endpoints after deployment

## Deployment Steps

### 1. Build and Push New Image
```bash
# Ensure on main branch with latest code
git checkout main
git pull origin main

# Build and push v1.2.0
./scripts/build-and-push.sh v1.2.0
```

### 2. Update ECS Service
```bash
# Option A: Force new deployment (uses latest task definition)
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# Option B: Create new task definition and update service
# (Use this if task definition changes are needed)
```

### 3. Monitor Deployment
```bash
# Watch service status
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].{status:status,deployments:deployments[*].{status:status,desiredCount:desiredCount,runningCount:runningCount}}'

# Watch CloudWatch logs
aws logs tail /ecs/facilitator-production --follow --region us-east-2

# Check health endpoint
curl https://facilitator.ultravioletadao.xyz/health
```

## Rollback Procedures

### Method 1: Rollback via ECS Service Update (FASTEST - 2-3 minutes)

```bash
# Rollback to previous task definition (facilitator-production:14)
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:14 \
  --force-new-deployment \
  --region us-east-2
```

**When to use**: If new deployment fails health checks or shows errors in logs

### Method 2: Manual Task Definition Rollback (if Method 1 fails)

```bash
# 1. Stop the failing deployment
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --desired-count 0 \
  --region us-east-2

# 2. Wait for tasks to stop (30 seconds)
sleep 30

# 3. Restart with old task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:14 \
  --desired-count 1 \
  --region us-east-2
```

**When to use**: If service becomes unresponsive or Method 1 doesn't work

### Method 3: Re-deploy Previous Image (NUCLEAR OPTION - 5-10 minutes)

```bash
# 1. Re-push v1.1.1 image as latest
docker pull 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.1.1
docker tag 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.1.1 \
  518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:latest

# 2. Push latest
docker push 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:latest

# 3. Force new deployment
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

**When to use**: If both Method 1 and 2 fail, or if ECS service is corrupted

## Health Check Commands

### Verify Service is Running
```bash
# Health endpoint (should return {"status":"healthy"})
curl https://facilitator.ultravioletadao.xyz/health

# Supported networks (should list all 14 networks)
curl https://facilitator.ultravioletadao.xyz/supported | jq

# Landing page (should show Ultravioleta branding)
curl -s https://facilitator.ultravioletadao.xyz/ | grep -i "ultravioleta"
```

### Check Logs for Errors
```bash
# Recent logs
aws logs tail /ecs/facilitator-production --since 10m --region us-east-2

# Filter for errors
aws logs tail /ecs/facilitator-production --since 10m --region us-east-2 --filter-pattern ERROR

# Follow live logs
aws logs tail /ecs/facilitator-production --follow --region us-east-2
```

### Monitor ALB Target Health
```bash
# Check target group health
aws elbv2 describe-target-health \
  --target-group-arn arn:aws:elasticloadbalancing:us-east-2:518898403364:targetgroup/facilitator-production/eb23fde229b27f7b \
  --region us-east-2
```

## Success Criteria

- [ ] Service status: ACTIVE
- [ ] Running tasks: 1/1
- [ ] Health endpoint returns: `{"status":"healthy"}`
- [ ] Supported networks endpoint returns 14+ networks
- [ ] Landing page shows Ultravioleta branding
- [ ] No ERROR logs in CloudWatch (last 5 minutes)
- [ ] ALB target health: healthy
- [ ] Response time < 500ms for health check

## Failure Indicators - ROLLBACK IMMEDIATELY

- ❌ Service fails to start tasks
- ❌ Tasks repeatedly fail health checks
- ❌ Health endpoint returns 5xx errors
- ❌ Landing page shows upstream "Hello" instead of Ultravioleta branding
- ❌ Custom networks missing from /supported endpoint
- ❌ ERROR logs in CloudWatch with payment failures
- ❌ Response time > 2s for health check

## Post-Deployment Verification

### Test Basic Endpoints
```bash
# Health
curl https://facilitator.ultravioletadao.xyz/health

# Networks
curl https://facilitator.ultravioletadao.xyz/supported

# Branding
curl https://facilitator.ultravioletadao.xyz/ | head -50
```

### Test Payment Flow (Testnet)
```bash
cd tests/integration
python test_usdc_payment.py --network base-sepolia
```

## Timeline Estimates

- **Deployment**: 2-3 minutes
- **Health check stabilization**: 1-2 minutes
- **Total deployment time**: 3-5 minutes
- **Rollback (Method 1)**: 2-3 minutes
- **Rollback (Method 2)**: 3-5 minutes
- **Rollback (Method 3)**: 5-10 minutes

## Emergency Contacts

- **AWS Console**: https://us-east-2.console.aws.amazon.com/ecs/v2/clusters/facilitator-production
- **CloudWatch Logs**: https://us-east-2.console.aws.amazon.com/cloudwatch/home?region=us-east-2#logsV2:log-groups/log-group/$252Fecs$252Ffacilitator-production
- **Service URL**: https://facilitator.ultravioletadao.xyz

## Backup Information

- **Previous stable version**: v1.1.1
- **Previous task definition**: facilitator-production:14
- **Previous image**: 518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator:v1.1.1
- **Git tag**: v1.1.1
- **Git commit**: a61b341

## Notes

- This deployment includes upstream security fixes that should improve Solana payment safety
- All customizations have been tested and preserved
- Rust edition 2021 ensures compatibility with current Rust version (1.82)
- Zero downtime deployment - new task starts before old task stops
- ALB health checks ensure traffic only routes to healthy containers

## Post-Rollback Actions (if needed)

1. Document what went wrong
2. Check logs for root cause
3. Test fix in local environment
4. Create new deployment plan
5. Notify team of incident and resolution
