# Production Deployment Checklist

This checklist ensures safe, reliable deployments to the Facilitator production environment.

## Pre-Deployment Checklist

### 1. Validate Secrets

Run the secrets validation script BEFORE every deployment:

```bash
cd terraform/environments/production
bash validate_secrets.sh us-east-2
```

Expected output: **All secrets validated successfully!**

If validation fails:
- Fix missing or invalid secrets in AWS Secrets Manager
- See `SECRETS_MANAGEMENT.md` for secret structure and creation instructions
- DO NOT proceed with deployment until validation passes

### 2. Review Code Changes

Check what has changed since last deployment:

```bash
# View recent commits
git log --oneline -10

# Check current branch
git branch --show-current

# Ensure you're on main branch for production
git checkout main
git pull origin main
```

### 3. Update Image Tag

Update the Docker image tag in `variables.tf`:

```bash
# Check currently deployed version
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].taskDefinition' --output text | \
  xargs aws ecs describe-task-definition --task-definition | \
  jq -r '.taskDefinition.containerDefinitions[0].image'

# Edit variables.tf
nano variables.tf
# Update: variable "image_tag" { default = "v1.X.Y" }
```

### 4. Validate Terraform Configuration

```bash
cd terraform/environments/production

# Initialize (if needed)
terraform init

# Validate syntax
terraform validate

# Expected output: "Success! The configuration is valid."
```

### 5. Review Terraform Plan

Generate and review the execution plan:

```bash
# Create plan
terraform plan -out=facilitator-prod.tfplan

# Review CAREFULLY:
# - Check which resources will be created/modified/destroyed
# - Verify task definition includes ALL required secrets
# - Ensure IAM policy includes all secret ARNs
# - Confirm no unexpected resource deletions
```

**RED FLAGS** (stop and investigate):
- Any resource marked for destruction (except old task definitions)
- Changes to VPC, subnets, security groups (unless intentional)
- Changes to ALB or target groups (unless intentional)
- Missing secrets in task definition

**SAFE CHANGES** (expected):
- New task definition revision
- ECS service update to use new task definition
- IAM policy updates for new secrets

## Deployment Steps

### 6. Apply Terraform Changes

```bash
# Apply the plan
terraform apply facilitator-prod.tfplan

# Monitor for errors
# If errors occur, DO NOT force through - investigate first
```

### 7. Force ECS Deployment

Terraform apply may not trigger a deployment if only environment variables changed:

```bash
# Force new deployment to pick up new task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# Expected output: deployment with status "PRIMARY"
```

### 8. Monitor Deployment

Watch the deployment progress:

```bash
# Check deployment status
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments'

# Expected: 2 deployments during rollout
# - PRIMARY (new): runningCount increasing
# - ACTIVE (old): runningCount decreasing
```

Wait for deployment to stabilize (typically 2-5 minutes). When complete:
- Only 1 deployment with status "PRIMARY"
- `runningCount` = `desiredCount`
- `rolloutState` = "COMPLETED"

### 9. Check CloudWatch Logs

Monitor application startup:

```bash
# Stream logs from new tasks
aws logs tail /ecs/facilitator-production \
  --follow \
  --region us-east-2

# Look for:
# - "Server started on 0.0.0.0:8080"
# - No ERROR or PANIC messages
# - Successful RPC connections
```

### 10. Verify Health Check

```bash
# Check ALB health
curl -s https://facilitator.ultravioletadao.xyz/health | jq

# Expected output:
# {"status":"healthy"}
```

### 11. Verify Supported Networks

```bash
# List supported networks
curl -s https://facilitator.ultravioletadao.xyz/supported | jq

# Verify ALL expected networks are present:
# - EVM: base, avalanche, polygon, optimism, celo, hyperevm, ethereum, arbitrum, unichain
# - Solana: solana, fogo
# - NEAR: near
# - Stellar: stellar (if deployed)
```

### 12. Test Payment Flow

Run integration tests for critical networks:

```bash
cd tests/integration

# Test mainnet networks (with small amounts!)
python test_usdc_payment.py --network base-mainnet --amount 0.01
python test_usdc_payment.py --network avalanche --amount 0.01

# Test testnet networks
python test_usdc_payment.py --network base-sepolia --amount 1.0
python test_usdc_payment.py --network avalanche-fuji --amount 1.0
```

Expected results:
- Health check: PASS
- Supported networks: PASS
- Payment verification: PASS
- Payment settlement: PASS

## Post-Deployment Checklist

### 13. Verify Metrics

Check CloudWatch metrics:

```bash
# ECS service metrics
aws cloudwatch get-metric-statistics \
  --namespace AWS/ECS \
  --metric-name CPUUtilization \
  --dimensions Name=ServiceName,Value=facilitator-production \
              Name=ClusterName,Value=facilitator-production \
  --start-time $(date -u -d '5 minutes ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 60 \
  --statistics Average \
  --region us-east-2

# Expected: CPUUtilization < 30% under normal load
```

### 14. Check for Errors

Review recent logs for errors:

```bash
# Last 100 log entries
aws logs tail /ecs/facilitator-production \
  --since 5m \
  --region us-east-2 | grep -i error

# Expected: No errors (empty output)
```

### 15. Verify Auto-Scaling

Check auto-scaling configuration:

```bash
aws application-autoscaling describe-scalable-targets \
  --service-namespace ecs \
  --resource-ids service/facilitator-production/facilitator-production \
  --region us-east-2

# Expected: min=1, max=3
```

### 16. Update Documentation

Document the deployment:

```bash
# Tag the release
git tag -a v1.X.Y -m "Production deployment $(date +%Y-%m-%d)"
git push origin v1.X.Y

# Update CHANGELOG.md (if applicable)
```

### 17. Notify Team

Post deployment notification:

```
Facilitator v1.X.Y deployed to production

Changes:
- [List key changes]

Verification:
✓ Health check passing
✓ All networks supported
✓ Integration tests passing
✓ No errors in logs

Rollback available if needed.
```

## Rollback Procedure

If deployment fails or issues are detected:

### Quick Rollback (revert to previous task definition)

```bash
# List recent task definitions
aws ecs list-task-definitions \
  --family-prefix facilitator-production \
  --sort DESC \
  --max-items 5 \
  --region us-east-2

# Update service to use previous task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:PREVIOUS_REVISION \
  --force-new-deployment \
  --region us-east-2

# Monitor rollback
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments'
```

### Full Rollback (revert Terraform state)

```bash
cd terraform/environments/production

# Revert variables.tf to previous image tag
git checkout HEAD~1 -- variables.tf

# Apply previous configuration
terraform plan -out=rollback.tfplan
terraform apply rollback.tfplan

# Force deployment
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

## Common Issues and Solutions

### Issue: Task fails to start with "ResourceInitializationError"

**Symptom:**
```
ResourceInitializationError: unable to pull secrets or registry auth
```

**Cause:** Missing IAM permissions for secrets or ECR.

**Solution:**
```bash
# 1. Verify secrets validation passed
bash validate_secrets.sh us-east-2

# 2. Check IAM execution role policy
terraform show | grep -A 30 "aws_iam_role_policy.secrets_access"

# 3. Verify all secrets in local.all_secret_arns
terraform console
> local.all_secret_arns

# 4. Re-apply Terraform to fix IAM policy
terraform apply -auto-approve
```

### Issue: Health check failing (504 Gateway Timeout)

**Symptom:**
```
curl https://facilitator.ultravioletadao.xyz/health
# Returns 504 or connection timeout
```

**Cause:** Application not starting or crashing on startup.

**Solution:**
```bash
# 1. Check CloudWatch logs for errors
aws logs tail /ecs/facilitator-production --follow --region us-east-2

# 2. Common causes:
#    - Missing environment variable (check task definition)
#    - Invalid RPC URL (check secrets)
#    - Out of memory (check task memory in variables.tf)

# 3. If memory issue, increase task_memory:
nano variables.tf
# Change: task_memory = 2048  ->  task_memory = 4096
terraform apply -auto-approve
```

### Issue: Network not supported after adding new chain

**Symptom:**
```bash
curl https://facilitator.ultravioletadao.xyz/supported
# New network missing from list
```

**Cause:** Missing environment variable or secret reference.

**Solution:**
```bash
# 1. Verify secret exists in AWS Secrets Manager
aws secretsmanager describe-secret \
  --secret-id facilitator-NETWORK-mainnet-keypair \
  --region us-east-2

# 2. Verify secret reference in secrets.tf
grep -A 5 "NETWORK" secrets.tf

# 3. Check task definition has environment variable
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 | \
  jq '.taskDefinition.containerDefinitions[0].secrets[] | select(.name | contains("NETWORK"))'

# 4. Re-apply Terraform and force deployment
terraform apply -auto-approve
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

### Issue: Terraform plan shows unexpected resource destruction

**Symptom:**
```
Plan: X to add, Y to change, Z to destroy
# Z > 0 and includes critical resources
```

**Cause:** State drift or configuration error.

**Solution:**
```bash
# DO NOT APPLY! Investigate first.

# 1. Check Terraform state
terraform show

# 2. Compare with AWS console
# Verify resources exist and match state

# 3. If state drift, refresh state:
terraform refresh

# 4. If configuration error, revert changes:
git diff
git checkout -- <problematic-file>

# 5. Re-run plan:
terraform plan

# Only apply when plan shows NO unexpected destruction
```

## Emergency Contacts

- **Infrastructure Lead:** [Your Name]
- **On-Call Engineer:** [On-call rotation]
- **AWS Support:** [Support case link]

## Cost Monitoring

After deployment, verify costs haven't increased unexpectedly:

```bash
# Check current month costs
aws ce get-cost-and-usage \
  --time-period Start=$(date -d '1 day ago' +%Y-%m-%d),End=$(date +%Y-%m-%d) \
  --granularity DAILY \
  --metrics BlendedCost \
  --group-by Type=SERVICE \
  --region us-east-1

# Expected: ~$1.50-2.00/day ($43-48/month)
```

If costs spike:
- Check for increased task count (auto-scaling)
- Verify NAT Gateway data transfer (should be minimal)
- Review CloudWatch Logs storage (7-day retention)

## Compliance and Audit

Record deployment details for audit trail:

- **Date/Time:** $(date -u +%Y-%m-%dT%H:%M:%SZ)
- **Deployed by:** $(whoami)
- **Git commit:** $(git rev-parse HEAD)
- **Image tag:** v1.X.Y
- **Task definition revision:** facilitator-production:XX
- **Pre-deployment validation:** PASSED/FAILED
- **Post-deployment tests:** PASSED/FAILED
- **Rollback executed:** YES/NO

## Automation (Future)

Consider automating this process with CI/CD:

```yaml
# .github/workflows/deploy-production.yml
name: Deploy to Production

on:
  push:
    tags:
      - 'v*.*.*'

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - name: Validate secrets
        run: bash terraform/environments/production/validate_secrets.sh

      - name: Terraform plan
        run: terraform plan -out=plan.tfplan

      - name: Terraform apply
        run: terraform apply plan.tfplan

      - name: Force ECS deployment
        run: |
          aws ecs update-service \
            --cluster facilitator-production \
            --service facilitator-production \
            --force-new-deployment

      - name: Run integration tests
        run: |
          cd tests/integration
          python test_facilitator.py
```

## Additional Resources

- [Secrets Management Guide](SECRETS_MANAGEMENT.md)
- [Terraform AWS Provider Docs](https://registry.terraform.io/providers/hashicorp/aws/latest/docs)
- [ECS Best Practices](https://docs.aws.amazon.com/AmazonECS/latest/bestpracticesguide/)
- [Wallet Rotation Procedures](/mnt/z/ultravioleta/dao/x402-rs/docs/WALLET_ROTATION.md)
