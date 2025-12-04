# NEAR Infrastructure - Terraform Commands Cheat Sheet

**Quick reference for deploying NEAR Protocol support**

---

## Pre-Deployment Checklist

```bash
# 1. Verify AWS credentials
aws sts get-caller-identity
# Expected: Account 518898403364, Region us-east-2

# 2. Verify current Terraform state
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production
terraform show | grep facilitator

# 3. Backup current state
aws s3 cp s3://facilitator-terraform-state/production/terraform.tfstate \
  ./terraform.tfstate.backup-$(date +%Y%m%d-%H%M%S)

# 4. Backup current task definition
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 > task-definition-backup-$(date +%Y%m%d-%H%M%S).json
```

---

## Step 1: Create NEAR Secrets

```bash
# Create mainnet secret (REPLACE YOUR_MAINNET_KEY)
aws secretsmanager create-secret \
  --name facilitator-near-mainnet-keypair \
  --description "NEAR mainnet keypair for facilitator payment settlements" \
  --secret-string '{"private_key":"ed25519:YOUR_MAINNET_KEY"}' \
  --region us-east-2 \
  --tags Key=Project,Value=facilitator Key=Environment,Value=production Key=ManagedBy,Value=terraform Key=Chain,Value=near

# Create testnet secret (REPLACE YOUR_TESTNET_KEY)
aws secretsmanager create-secret \
  --name facilitator-near-testnet-keypair \
  --description "NEAR testnet keypair for facilitator payment settlements" \
  --secret-string '{"private_key":"ed25519:YOUR_TESTNET_KEY"}' \
  --region us-east-2 \
  --tags Key=Project,Value=facilitator Key=Environment,Value=production Key=ManagedBy,Value=terraform Key=Chain,Value=near

# Verify secrets created
aws secretsmanager list-secrets --region us-east-2 --filters "Key=name,Values=facilitator-near"

# Get secret ARNs (save these)
aws secretsmanager describe-secret \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2 \
  --query 'ARN' \
  --output text

aws secretsmanager describe-secret \
  --secret-id facilitator-near-testnet-keypair \
  --region us-east-2 \
  --query 'ARN' \
  --output text
```

---

## Step 2: Update Terraform Files

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Backup current main.tf
cp main.tf main.tf.backup-$(date +%Y%m%d-%H%M%S)

# Option A: Use pre-generated file (RECOMMENDED)
cp main-near-updated.tf main.tf

# Option B: Manually edit main.tf (see deployment guide)
# - Add NEAR secret data sources (after line 34)
# - Update IAM policy (lines 391-412)
# - Add environment variables (after line 523)
# - Add secrets (after line 562)

# Verify CloudWatch metrics file exists
ls -lh cloudwatch-near-metrics.tf
```

---

## Step 3: Terraform Plan

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Initialize (if needed)
terraform init

# Format files
terraform fmt

# Validate syntax
terraform validate
# Expected: "Success! The configuration is valid."

# Generate plan
terraform plan -out=facilitator-near-integration.tfplan

# Review plan output
# Expected: Plan: 5 to add, 2 to change, 0 to destroy.

# Check what will be added
grep "# aws_" facilitator-near-integration.tfplan || terraform show facilitator-near-integration.tfplan | grep "will be created\|will be updated"
```

### Expected Plan Output

```
Terraform will perform the following actions:

  # data.aws_secretsmanager_secret.near_mainnet_keypair will be read during apply
  # data.aws_secretsmanager_secret.near_testnet_keypair will be read during apply

  # aws_iam_role_policy.secrets_access will be updated in-place
  ~ resource "aws_iam_role_policy" "secrets_access" {
      ~ policy = jsonencode(
          ~ {
              ~ Statement = [
                  ~ {
                      ~ Resource = [
                          + "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-mainnet-keypair-*",
                          + "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-testnet-keypair-*",
                        ]
                    }
                ]
            }
        )
    }

  # aws_ecs_task_definition.facilitator will be updated in-place
  ~ resource "aws_ecs_task_definition" "facilitator" {
      ~ container_definitions    = jsonencode(
          ~ [
              ~ {
                  ~ environment = [
                      + {
                          + name  = "RPC_URL_NEAR_MAINNET"
                          + value = "https://rpc.mainnet.near.org"
                        },
                      + {
                          + name  = "RPC_URL_NEAR_TESTNET"
                          + value = "https://rpc.testnet.near.org"
                        },
                    ]
                  ~ secrets     = [
                      + {
                          + name      = "NEAR_PRIVATE_KEY_MAINNET"
                          + valueFrom = "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-mainnet-keypair:private_key::"
                        },
                      + {
                          + name      = "NEAR_PRIVATE_KEY_TESTNET"
                          + valueFrom = "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-testnet-keypair:private_key::"
                        },
                    ]
                }
            ]
        )
    }

  # aws_cloudwatch_log_metric_filter.near_settlement_success will be created
  # aws_cloudwatch_log_metric_filter.near_settlement_failure will be created
  # aws_cloudwatch_log_metric_filter.near_rpc_error will be created
  # aws_cloudwatch_log_metric_filter.near_verification_success will be created
  # aws_cloudwatch_log_metric_filter.near_verification_failure will be created
  # aws_cloudwatch_metric_alarm.near_settlement_failure_rate will be created
  # aws_cloudwatch_metric_alarm.near_rpc_errors will be created
  # aws_cloudwatch_dashboard.near_operations will be created

Plan: 5 to add, 2 to change, 0 to destroy.
```

---

## Step 4: Apply Terraform Changes

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# Apply the plan (TIMESTAMP: YYYY-MM-DD HH:MM:SS)
echo "Deployment started at: $(date)"
terraform apply facilitator-near-integration.tfplan

# Expected output:
# Apply complete! Resources: 5 added, 2 changed, 0 destroyed.

# Verify outputs
terraform output

# Check new task definition revision
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 \
  --query 'taskDefinition.revision'
```

---

## Step 5: Deploy to ECS

```bash
# Force new deployment with updated task definition (TIMESTAMP: YYYY-MM-DD HH:MM:SS)
echo "ECS deployment started at: $(date)"
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# Watch deployment progress (press Ctrl+C to stop)
watch -n 5 'aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query "services[0].[status,runningCount,desiredCount,deployments[0].status]" \
  --output table'

# Alternative: Check deployment status once
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments' \
  --output table

# Get latest task ARN
TASK_ARN=$(aws ecs list-tasks \
  --cluster facilitator-production \
  --service facilitator-production \
  --region us-east-2 \
  --query 'taskArns[0]' \
  --output text)

echo "Latest task: $TASK_ARN"

# View task logs (press Ctrl+C to stop)
aws logs tail /ecs/facilitator-production \
  --follow \
  --since 5m \
  --region us-east-2
```

---

## Step 6: Validation

```bash
# 1. Health check (TIMESTAMP: YYYY-MM-DD HH:MM:SS)
echo "Health check at: $(date)"
curl -s https://facilitator.ultravioletadao.xyz/health | jq

# Expected: {"status":"healthy"}

# 2. Check supported networks
curl -s https://facilitator.ultravioletadao.xyz/supported | \
  jq '.networks[] | select(.network | contains("near"))'

# Expected:
# {
#   "network": "near-mainnet",
#   "schemes": ["x402-near"],
#   ...
# }

# 3. Verify NEAR environment variables in logs
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "NEAR_PRIVATE_KEY" \
  --region us-east-2 \
  --max-items 5 \
  --query 'events[*].message' \
  --output text

# Expected: "NEAR_PRIVATE_KEY_MAINNET: Found in environment"

# 4. Check CloudWatch dashboard
terraform output near_dashboard_url
# Open URL in browser

# 5. Verify IAM permissions
aws iam get-role-policy \
  --role-name facilitator-production-ecs-execution \
  --policy-name secrets-access \
  --region us-east-2 | \
  jq '.PolicyDocument.Statement[0].Resource' | \
  grep near

# Expected: Both NEAR secret ARNs listed

# 6. Test NEAR verification endpoint
curl -s https://facilitator.ultravioletadao.xyz/verify?network=near-testnet | jq

# Should return NEAR schema (not an error)
```

---

## Rollback (If Needed)

### Quick Rollback (2 minutes)

```bash
# Get previous task definition revision
aws ecs list-task-definitions \
  --family-prefix facilitator-production \
  --region us-east-2 \
  --sort DESC | head -5

# Rollback to previous revision (e.g., revision 42)
PREVIOUS_REVISION=42
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:${PREVIOUS_REVISION} \
  --region us-east-2

# Monitor rollback
watch -n 5 'aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query "services[0].deployments" \
  --output table'
```

### Full Rollback (10 minutes)

```bash
cd Z:\ultravioleta\dao\x402-rs\terraform\environments\production

# 1. Restore main.tf
cp main.tf.backup-YYYYMMDD-HHMMSS main.tf

# 2. Remove CloudWatch metrics file
rm cloudwatch-near-metrics.tf

# 3. Re-initialize and plan
terraform init
terraform plan -out=rollback.tfplan

# 4. Apply rollback
terraform apply rollback.tfplan

# 5. Force ECS deployment
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# 6. (Optional) Delete NEAR secrets (30-day recovery window)
aws secretsmanager delete-secret \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2

aws secretsmanager delete-secret \
  --secret-id facilitator-near-testnet-keypair \
  --region us-east-2
```

---

## Troubleshooting Commands

### Issue: Secret Not Found

```bash
# List all secrets
aws secretsmanager list-secrets --region us-east-2

# Describe NEAR secrets
aws secretsmanager describe-secret \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2

# Verify secret value format
aws secretsmanager get-secret-value \
  --secret-id facilitator-near-mainnet-keypair \
  --region us-east-2 \
  --query 'SecretString' \
  --output text | jq
```

### Issue: IAM Permission Denied

```bash
# Check IAM policy
aws iam get-role-policy \
  --role-name facilitator-production-ecs-execution \
  --policy-name secrets-access \
  --region us-east-2

# List IAM policies attached to role
aws iam list-role-policies \
  --role-name facilitator-production-ecs-execution \
  --region us-east-2

# Verify task execution role ARN
terraform state show aws_iam_role.ecs_task_execution | grep arn
```

### Issue: Task Fails to Start

```bash
# Get task failure reason
aws ecs describe-tasks \
  --cluster facilitator-production \
  --tasks $(aws ecs list-tasks \
    --cluster facilitator-production \
    --service facilitator-production \
    --region us-east-2 \
    --query 'taskArns[0]' \
    --output text) \
  --region us-east-2 \
  --query 'tasks[0].stoppedReason'

# Check task logs for errors
aws logs tail /ecs/facilitator-production \
  --since 10m \
  --region us-east-2 | grep -i "error\|panic\|fatal"
```

### Issue: Health Check Failing

```bash
# Check ALB target health
aws elbv2 describe-target-health \
  --target-group-arn $(terraform output -raw alb_arn | sed 's/loadbalancer/targetgroup/') \
  --region us-east-2

# Check ECS service events
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].events[0:10]'

# Test health endpoint directly
curl -v https://facilitator.ultravioletadao.xyz/health
```

### Issue: NEAR RPC Timeout

```bash
# Test NEAR RPC directly
curl -s https://rpc.mainnet.near.org/status | jq

# Check application logs for RPC errors
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "RPC\|timeout\|NEAR" \
  --region us-east-2 \
  --max-items 20

# Check CloudWatch NEAR metrics
aws cloudwatch get-metric-statistics \
  --namespace Facilitator/NEAR \
  --metric-name NEARRPCError \
  --start-time $(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 300 \
  --statistics Sum \
  --region us-east-2
```

---

## Monitoring Commands

### CloudWatch Metrics

```bash
# View NEAR settlement success count (last hour)
aws cloudwatch get-metric-statistics \
  --namespace Facilitator/NEAR \
  --metric-name NEARSettlementSuccess \
  --start-time $(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 300 \
  --statistics Sum \
  --region us-east-2

# View NEAR settlement failure count (last hour)
aws cloudwatch get-metric-statistics \
  --namespace Facilitator/NEAR \
  --metric-name NEARSettlementFailure \
  --start-time $(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 300 \
  --statistics Sum \
  --region us-east-2

# List all NEAR alarms
aws cloudwatch describe-alarms \
  --alarm-name-prefix facilitator-near \
  --region us-east-2 \
  --query 'MetricAlarms[*].[AlarmName,StateValue]' \
  --output table
```

### Log Queries

```bash
# Search NEAR logs (last 1 hour)
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "near" \
  --start-time $(($(date +%s) - 3600))000 \
  --region us-east-2 \
  --query 'events[*].message' \
  --output text

# Count NEAR settlement events (last 24 hours)
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "settlement\|NEAR" \
  --start-time $(($(date +%s) - 86400))000 \
  --region us-east-2 \
  --query 'length(events)'

# Find NEAR errors (last 1 hour)
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "ERROR\|PANIC" \
  --start-time $(($(date +%s) - 3600))000 \
  --region us-east-2 \
  --query 'events[*].message' \
  --output text | grep -i near
```

---

## Cost Monitoring

```bash
# Get facilitator costs (current month)
aws ce get-cost-and-usage \
  --time-period Start=$(date -d "$(date +%Y-%m-01)" +%Y-%m-%d),End=$(date +%Y-%m-%d) \
  --granularity MONTHLY \
  --metrics BlendedCost \
  --group-by Type=TAG,Key=Project \
  --filter file://<(echo '{
    "Tags": {
      "Key": "Project",
      "Values": ["facilitator"]
    }
  }') \
  --region us-east-1

# Get Secrets Manager costs (current month)
aws ce get-cost-and-usage \
  --time-period Start=$(date -d "$(date +%Y-%m-01)" +%Y-%m-%d),End=$(date +%Y-%m-%d) \
  --granularity MONTHLY \
  --metrics BlendedCost \
  --group-by Type=SERVICE \
  --filter file://<(echo '{
    "Dimensions": {
      "Key": "SERVICE",
      "Values": ["AWS Secrets Manager"]
    }
  }') \
  --region us-east-1
```

---

## Useful Aliases (Add to ~/.bashrc)

```bash
# Facilitator shortcuts
alias fac-logs='aws logs tail /ecs/facilitator-production --follow --region us-east-2'
alias fac-health='curl -s https://facilitator.ultravioletadao.xyz/health | jq'
alias fac-status='aws ecs describe-services --cluster facilitator-production --services facilitator-production --region us-east-2 --query "services[0].[status,runningCount,desiredCount]" --output table'
alias fac-tasks='aws ecs list-tasks --cluster facilitator-production --service facilitator-production --region us-east-2'
alias fac-deploy='aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2'
alias fac-near-logs='aws logs filter-log-events --log-group-name /ecs/facilitator-production --filter-pattern "near" --region us-east-2 --max-items 20 --query "events[*].message" --output text'

# Terraform shortcuts
alias tf='terraform'
alias tfp='terraform plan -out=facilitator.tfplan'
alias tfa='terraform apply facilitator.tfplan'
alias tfs='terraform show'
alias tfo='terraform output'
```

---

## Quick Reference Card

| Task | Command |
|------|---------|
| Create secret | `aws secretsmanager create-secret --name facilitator-near-mainnet-keypair ...` |
| Plan changes | `terraform plan -out=facilitator-near.tfplan` |
| Apply changes | `terraform apply facilitator-near.tfplan` |
| Deploy to ECS | `aws ecs update-service --force-new-deployment ...` |
| Watch deployment | `watch -n 5 'aws ecs describe-services ...'` |
| View logs | `aws logs tail /ecs/facilitator-production --follow` |
| Health check | `curl https://facilitator.ultravioletadao.xyz/health` |
| Rollback | `aws ecs update-service --task-definition facilitator-production:PREV_REV ...` |
| Dashboard URL | `terraform output near_dashboard_url` |

---

**Last Updated**: 2025-12-03
**Terraform Version**: 1.0+
**AWS CLI Version**: 2.x
