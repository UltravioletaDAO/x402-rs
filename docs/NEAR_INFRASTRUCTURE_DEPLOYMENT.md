# NEAR Protocol Infrastructure Deployment Guide

**Date**: 2025-12-03
**Purpose**: Step-by-step guide for deploying NEAR Protocol support to AWS infrastructure
**Estimated Time**: 30-45 minutes
**Downtime**: Zero (blue-green deployment via ECS)

---

## Prerequisites

### 1. NEAR Wallet Setup

Before deploying infrastructure, you must create and fund NEAR wallets:

**Mainnet Wallet**:
- Create wallet at: https://wallet.near.org
- Export private key (Settings > Security & Recovery > Export Private Key)
- Format: `ed25519:<base58_encoded_key>`
- Fund with at least 5 NEAR tokens for gas fees

**Testnet Wallet**:
- Create wallet at: https://testnet.mynearwallet.com
- Export private key (same process as mainnet)
- Fund with testnet NEAR from: https://near-faucet.io

### 2. AWS Credentials

Ensure you have AWS CLI configured with appropriate permissions:
```bash
aws sts get-caller-identity
# Should return: Account 518898403364, Region us-east-2
```

### 3. Required Permissions

Your AWS user/role must have:
- `secretsmanager:CreateSecret`
- `secretsmanager:UpdateSecret`
- `secretsmanager:TagResource`
- `ecs:UpdateService`
- `iam:GetRole`
- `iam:PassRole`

### 4. Backup Current State

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Backup current Terraform state
aws s3 cp s3://facilitator-terraform-state/production/terraform.tfstate \
  ./terraform.tfstate.backup-$(date +%Y%m%d-%H%M%S)

# Backup current task definition
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 > task-definition-backup-$(date +%Y%m%d-%H%M%S).json
```

---

## Phase 1: Create AWS Secrets (15 minutes)

### Step 1.1: Create NEAR Mainnet Keypair Secret

**CRITICAL**: Replace `YOUR_MAINNET_PRIVATE_KEY` with your actual mainnet private key.

```bash
aws secretsmanager create-secret \
  --name facilitator-near-mainnet-keypair \
  --description "NEAR mainnet keypair for facilitator payment settlements" \
  --secret-string '{"private_key":"ed25519:YOUR_MAINNET_PRIVATE_KEY"}' \
  --region us-east-2 \
  --tags Key=Project,Value=facilitator Key=Environment,Value=production Key=ManagedBy,Value=terraform Key=Chain,Value=near
```

**Expected Output**:
```json
{
    "ARN": "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-mainnet-keypair-XXXXXX",
    "Name": "facilitator-near-mainnet-keypair",
    "VersionId": "..."
}
```

**Verify Secret**:
```bash
aws secretsmanager get-secret-value \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2 \
  --query 'SecretString' \
  --output text | jq
```

### Step 1.2: Create NEAR Testnet Keypair Secret

**CRITICAL**: Replace `YOUR_TESTNET_PRIVATE_KEY` with your actual testnet private key.

```bash
aws secretsmanager create-secret \
  --name facilitator-near-testnet-keypair \
  --description "NEAR testnet keypair for facilitator payment settlements" \
  --secret-string '{"private_key":"ed25519:YOUR_TESTNET_PRIVATE_KEY"}' \
  --region us-east-2 \
  --tags Key=Project,Value=facilitator Key=Environment,Value=production Key=ManagedBy,Value=terraform Key=Chain,Value=near
```

**Verify Secret**:
```bash
aws secretsmanager get-secret-value \
  --secret-id facilitator-near-testnet-keypair \
  --region us-east-2 \
  --query 'SecretString' \
  --output text | jq
```

### Step 1.3: Capture Secret ARNs

**IMPORTANT**: Save these ARNs - you'll need them in Phase 2.

```bash
# Get mainnet secret ARN
export NEAR_MAINNET_ARN=$(aws secretsmanager describe-secret \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2 \
  --query 'ARN' \
  --output text)

# Get testnet secret ARN
export NEAR_TESTNET_ARN=$(aws secretsmanager describe-secret \
  --secret-id facilitator-near-testnet-keypair \
  --region us-east-2 \
  --query 'ARN' \
  --output text)

# Display ARNs
echo "Mainnet ARN: $NEAR_MAINNET_ARN"
echo "Testnet ARN: $NEAR_TESTNET_ARN"
```

**Checkpoint**: You should now have 2 new secrets visible in AWS Console:
- https://console.aws.amazon.com/secretsmanager/home?region=us-east-2

---

## Phase 2: Update Terraform Configuration (10 minutes)

### Step 2.1: Review Updated Terraform Files

Three files need to be updated:
1. `main.tf` - Add NEAR secret data sources + update IAM + task definition
2. `cloudwatch-near-metrics.tf` - NEW FILE for NEAR monitoring
3. `outputs.tf` - Add NEAR-specific outputs (optional)

**Option A: Apply Pre-Generated Files** (Recommended)

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Review the updated main.tf
diff main.tf main-near-updated.tf

# If the diff looks correct, replace the file
cp main.tf main.tf.backup-$(date +%Y%m%d-%H%M%S)
cp main-near-updated.tf main.tf

# Add CloudWatch metrics file (already generated)
# File: cloudwatch-near-metrics.tf is ready to use
```

**Option B: Manual Updates**

If you prefer to manually edit `main.tf`:

1. **Add NEAR secret data sources** (after line 34):
```hcl
# NEAR keypair secrets
data "aws_secretsmanager_secret" "near_mainnet_keypair" {
  name = "facilitator-near-mainnet-keypair"
}

data "aws_secretsmanager_secret" "near_testnet_keypair" {
  name = "facilitator-near-testnet-keypair"
}
```

2. **Update IAM policy** (lines 391-412) - Add to `Resource` array:
```hcl
data.aws_secretsmanager_secret.near_mainnet_keypair.arn,
data.aws_secretsmanager_secret.near_testnet_keypair.arn
```

3. **Add environment variables** (after line 523 in container `environment` array):
```hcl
{
  name  = "RPC_URL_NEAR_MAINNET"
  value = "https://rpc.mainnet.near.org"
},
{
  name  = "RPC_URL_NEAR_TESTNET"
  value = "https://rpc.testnet.near.org"
}
```

4. **Add secrets** (after line 562 in container `secrets` array):
```hcl
{
  name      = "NEAR_PRIVATE_KEY_MAINNET"
  valueFrom = "${data.aws_secretsmanager_secret.near_mainnet_keypair.arn}:private_key::"
},
{
  name      = "NEAR_PRIVATE_KEY_TESTNET"
  valueFrom = "${data.aws_secretsmanager_secret.near_testnet_keypair.arn}:private_key::"
}
```

### Step 2.2: Validate Terraform Configuration

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Initialize (if needed)
terraform init

# Format files
terraform fmt

# Validate syntax
terraform validate

# Expected output: "Success! The configuration is valid."
```

### Step 2.3: Generate Terraform Plan

```bash
# Generate plan
terraform plan -out=facilitator-near-integration.tfplan

# Review the plan carefully - you should see:
# - 2 new data sources (NEAR secrets)
# - 1 IAM policy update (secrets_access)
# - 1 task definition update (environment + secrets)
# - 5 new CloudWatch resources (metrics filters + alarms + dashboard)
# - No resources destroyed
```

**Expected Changes Summary**:
```
Plan: 5 to add, 2 to change, 0 to destroy.
```

**Review Checklist**:
- [ ] IAM policy includes both NEAR secret ARNs
- [ ] Task definition has 2 new environment variables
- [ ] Task definition has 2 new secrets (valueFrom references)
- [ ] No resources being destroyed
- [ ] CloudWatch resources are being created

---

## Phase 3: Apply Infrastructure Changes (10 minutes)

### Step 3.1: Apply Terraform Plan

**Timestamp**: 2025-12-03 [CURRENT_TIME]

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Apply the plan
terraform apply facilitator-near-integration.tfplan
```

**Expected Output**:
```
Apply complete! Resources: 5 added, 2 changed, 0 destroyed.
```

### Step 3.2: Force ECS Service Deployment

Terraform will create a new task definition revision, but won't automatically deploy it. Force a deployment:

```bash
# Trigger rolling deployment
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# Expected output:
# "service": {
#   "status": "ACTIVE",
#   "desiredCount": 1,
#   ...
# }
```

**Timestamp**: Deployment initiated at [CURRENT_TIME + 5 minutes]

---

## Phase 4: Monitor Deployment (5-10 minutes)

### Step 4.1: Watch ECS Task Replacement

```bash
# Watch task status
watch -n 5 'aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query "services[0].[runningCount,desiredCount,deployments]" \
  --output table'
```

**What to expect**:
1. **0-2 minutes**: New task starts, health checks begin
2. **2-5 minutes**: New task passes health checks, becomes healthy
3. **5-7 minutes**: Old task drains connections, shuts down
4. **7-10 minutes**: Deployment complete, 1 new task running

**Checkpoint**: When `runningCount` == `desiredCount` == 1, deployment is complete.

### Step 4.2: Check Task Logs

```bash
# Get the latest task ID
TASK_ARN=$(aws ecs list-tasks \
  --cluster facilitator-production \
  --service facilitator-production \
  --region us-east-2 \
  --query 'taskArns[0]' \
  --output text)

# View logs (last 50 lines)
aws logs tail /ecs/facilitator-production \
  --follow \
  --since 5m \
  --region us-east-2
```

**Look for**:
- `INFO facilitator initialized` - Service started successfully
- `NEAR_PRIVATE_KEY_MAINNET: Found in environment` - Secret loaded
- `NEAR_PRIVATE_KEY_TESTNET: Found in environment` - Secret loaded
- `RPC_URL_NEAR_MAINNET: https://rpc.mainnet.near.org` - Public RPC configured
- `RPC_URL_NEAR_TESTNET: https://rpc.testnet.near.org` - Public RPC configured

**Red flags** (should NOT see):
- `ERROR: Failed to load NEAR secret`
- `PANIC: Secret not found`
- `Connection refused: rpc.mainnet.near.org`

### Step 4.3: Verify Health Endpoint

```bash
# Test health endpoint
curl -s https://facilitator.ultravioletadao.xyz/health | jq

# Expected output:
# {
#   "status": "healthy"
# }
```

**Timestamp**: Health check passed at [CURRENT_TIME + 10 minutes]

---

## Phase 5: Validation Testing (5 minutes)

### Step 5.1: Check Supported Networks

```bash
# List supported networks - should include NEAR
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.networks[] | select(.network | contains("near"))'

# Expected output:
# {
#   "network": "near-mainnet",
#   "schemes": ["x402-near"],
#   ...
# }
# {
#   "network": "near-testnet",
#   "schemes": ["x402-near"],
#   ...
# }
```

### Step 5.2: Test NEAR Verification Endpoint

```bash
# Test verification schema
curl -s https://facilitator.ultravioletadao.xyz/verify?network=near-testnet | jq

# Should return NEAR-specific schema (not an error)
```

### Step 5.3: Check CloudWatch Dashboard

```bash
# Get dashboard URL
terraform output near_dashboard_url

# Open in browser - should show NEAR metrics dashboard
```

**Checkpoint**: Dashboard URL is accessible and shows empty metrics (no data yet, which is expected).

---

## Phase 6: Post-Deployment Verification (5 minutes)

### Step 6.1: Verify Secrets Access

```bash
# Test that ECS task can read NEAR secrets (check logs)
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "NEAR_PRIVATE_KEY" \
  --region us-east-2 \
  --max-items 5

# Should show environment variable was loaded (value not logged)
```

### Step 6.2: Verify IAM Permissions

```bash
# Check task execution role has updated policy
aws iam get-role-policy \
  --role-name facilitator-production-ecs-execution \
  --policy-name secrets-access \
  --region us-east-2 | jq '.PolicyDocument.Statement[0].Resource'

# Should include both NEAR secret ARNs
```

### Step 6.3: Cost Verification

```bash
# Check current AWS costs (should not increase)
aws ce get-cost-and-usage \
  --time-period Start=2025-12-01,End=2025-12-04 \
  --granularity DAILY \
  --metrics BlendedCost \
  --filter file://<(echo '{
    "Tags": {
      "Key": "Project",
      "Values": ["facilitator"]
    }
  }') \
  --region us-east-1

# Expected: Same ~$1.50/day as before (no cost increase)
```

---

## Rollback Procedure (If Issues Occur)

### Option A: Revert to Previous Task Definition

```bash
# List task definition revisions
aws ecs list-task-definitions \
  --family-prefix facilitator-production \
  --region us-east-2 \
  --sort DESC

# Rollback to previous revision (e.g., revision 42)
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:42 \
  --region us-east-2

# Monitor rollback
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments'
```

### Option B: Revert Terraform State

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Restore backup
cp terraform.tfstate.backup-YYYYMMDD-HHMMSS terraform.tfstate

# Revert main.tf
cp main.tf.backup-YYYYMMDD-HHMMSS main.tf

# Delete CloudWatch resources
rm cloudwatch-near-metrics.tf

# Apply old configuration
terraform init
terraform plan -out=rollback.tfplan
terraform apply rollback.tfplan
```

### Option C: Delete NEAR Secrets (Nuclear Option)

```bash
# Delete secrets (30-day recovery window)
aws secretsmanager delete-secret \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2

aws secretsmanager delete-secret \
  --secret-id facilitator-near-testnet-keypair \
  --region us-east-2

# Revert to Phase 2 and start over
```

---

## Troubleshooting

### Issue 1: "Secret not found" Error

**Symptom**: Task fails to start, logs show `SecretNotFoundException`

**Cause**: IAM policy doesn't include NEAR secret ARNs

**Fix**:
```bash
# Verify IAM policy
aws iam get-role-policy \
  --role-name facilitator-production-ecs-execution \
  --policy-name secrets-access \
  --region us-east-2

# If missing, re-run terraform apply
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production
terraform apply
```

### Issue 2: "Invalid NEAR private key" Error

**Symptom**: Task starts but logs show NEAR key parsing errors

**Cause**: Private key format incorrect in AWS Secrets Manager

**Fix**:
```bash
# Verify secret format
aws secretsmanager get-secret-value \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2 \
  --query 'SecretString' \
  --output text

# Should be: {"private_key":"ed25519:<base58_key>"}

# Update if incorrect
aws secretsmanager update-secret \
  --secret-id facilitator-near-mainnet-keypair \
  --secret-string '{"private_key":"ed25519:CORRECT_KEY_HERE"}' \
  --region us-east-2

# Force task restart
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

### Issue 3: NEAR RPC Timeout

**Symptom**: NEAR transactions fail with timeout errors

**Cause**: Public NEAR RPC is rate-limiting or slow

**Fix**: Upgrade to premium NEAR RPC (Infura, Alchemy, etc.)
```bash
# Add premium RPC to existing secret
aws secretsmanager update-secret \
  --secret-id facilitator-rpc-mainnet \
  --secret-string "$(aws secretsmanager get-secret-value \
    --secret-id facilitator-rpc-mainnet \
    --query SecretString \
    --output text | jq '. + {"near": "https://near-mainnet.infura.io/v3/YOUR_API_KEY"}')" \
  --region us-east-2

# Update task definition to use premium RPC
# (Update main.tf to reference facilitator-rpc-mainnet:near::)
```

### Issue 4: Task Fails Health Checks

**Symptom**: Task starts but keeps restarting, never becomes healthy

**Cause**: Application crash during NEAR initialization

**Fix**:
```bash
# Check application logs for stack trace
aws logs tail /ecs/facilitator-production \
  --follow \
  --since 10m \
  --region us-east-2 | grep -i "near\|panic\|error"

# Common issues:
# - Invalid private key format
# - RPC endpoint unreachable
# - Missing dependencies in Docker image

# Verify facilitator was rebuilt with NEAR support
aws ecr describe-images \
  --repository-name facilitator \
  --region us-east-2 \
  --query 'imageDetails[0].[imagePushedAt,imageTags]'
```

---

## Success Criteria Checklist

- [ ] Both NEAR secrets created and verified in AWS Secrets Manager
- [ ] Terraform plan shows 5 added, 2 changed, 0 destroyed
- [ ] Terraform apply completed without errors
- [ ] ECS service deployment completed (1/1 tasks running)
- [ ] Health endpoint returns `{"status":"healthy"}`
- [ ] `/supported` endpoint includes `near-mainnet` and `near-testnet`
- [ ] CloudWatch dashboard accessible and shows NEAR metrics
- [ ] Application logs show NEAR keys loaded successfully
- [ ] No cost increase ($43-48/month maintained)
- [ ] Old task definition backed up
- [ ] Terraform state backed up

---

## Post-Deployment Tasks

### Enable CloudWatch Alarms (Optional)

Add SNS topic for alert notifications:

```bash
# Create SNS topic
aws sns create-topic \
  --name facilitator-near-alerts \
  --region us-east-2

# Subscribe your email
aws sns subscribe \
  --topic-arn arn:aws:sns:us-east-2:518898403364:facilitator-near-alerts \
  --protocol email \
  --notification-endpoint your-email@domain.com \
  --region us-east-2

# Update cloudwatch-near-metrics.tf to add SNS ARN to alarm_actions
# Then re-run terraform apply
```

### Schedule NEAR Wallet Balance Monitoring

Create Lambda function to alert when NEAR balance is low:

```bash
# See: guides/WALLET_BALANCE_MONITORING.md (to be created)
```

### Document NEAR Key Rotation Procedure

Add NEAR-specific steps to `docs/WALLET_ROTATION.md`:
- Export new NEAR keypair
- Update AWS Secrets Manager
- Test with testnet first
- Update mainnet secret
- Force task restart

---

## Maintenance

### Monthly Tasks

- [ ] Check NEAR wallet balances (mainnet and testnet)
- [ ] Review CloudWatch NEAR metrics for anomalies
- [ ] Verify NEAR RPC endpoint performance
- [ ] Check for NEAR Protocol updates

### Quarterly Tasks

- [ ] Rotate NEAR testnet keypair (practice for mainnet rotation)
- [ ] Review NEAR settlement costs and optimize if needed
- [ ] Update NEAR RPC endpoints if better providers available

---

## Contact & Support

**Infrastructure Issues**:
- AWS Support: https://console.aws.amazon.com/support
- Terraform: Review docs/DEPLOYMENT.md

**NEAR Integration Issues**:
- NEAR Docs: https://docs.near.org
- NEAR Discord: https://discord.gg/near

**Facilitator Code Issues**:
- Invoke `aegis-rust-architect` agent for application-level NEAR support
- GitHub Issues: https://github.com/UltravioletaDAO/x402-rs/issues

---

**Deployment Guide Version**: 1.0
**Last Updated**: 2025-12-03
**Tested On**: AWS ECS Fargate, us-east-2
**Estimated Total Time**: 45 minutes
