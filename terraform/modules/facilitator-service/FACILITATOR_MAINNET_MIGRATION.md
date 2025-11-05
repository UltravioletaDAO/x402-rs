# Facilitator Mainnet Migration - Complete

## Summary

Successfully migrated the facilitator ECS task definition from testnet to mainnet private key using Terraform.

## What Was Done

### 1. Updated Terraform Configuration
**File**: `Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate\main.tf` (line 62)

**Change**:
```hcl
# OLD (testnet):
name = each.key == "facilitator" ? "karmacadabra-facilitator-testnet" : "karmacadabra-${each.key}"

# NEW (mainnet):
name = each.key == "facilitator" ? "karmacadabra-facilitator-mainnet" : "karmacadabra-${each.key}"
```

### 2. Applied Terraform Changes
The facilitator resources were already in Terraform state, so we simply applied the configuration change:

```bash
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate

# Applied the task definition change
terraform apply -target="aws_ecs_task_definition.agents[\"facilitator\"]" -auto-approve

# Updated the ECS service to use new task definition
terraform apply -target="aws_ecs_service.agents[\"facilitator\"]" -auto-approve
```

### 3. Results
- **Old task definition**: karmacadabra-prod-facilitator:13 (testnet key)
- **New task definition**: karmacadabra-prod-facilitator:16 (mainnet key)
- **ECS Service**: Updated to use revision 16
- **Deployment Status**: Healthy (both old and new tasks are running during rollout)

## Verification

### Task Definition Secret ARNs

**Revision 16 (Current - Mainnet)**:
```json
[
    {
        "name": "EVM_PRIVATE_KEY",
        "valueFrom": "arn:aws:secretsmanager:us-east-1:518898403364:secret:karmacadabra-facilitator-mainnet-WTvZkf:private_key::"
    },
    {
        "name": "SOLANA_PRIVATE_KEY",
        "valueFrom": "arn:aws:secretsmanager:us-east-1:518898403364:secret:karmacadabra-solana-keypair-yWgz6P:private_key::"
    }
]
```

**Revision 15 (Previous - You created manually)**:
- Used mainnet key (your manual fix)

**Revision 13 (Old - Testnet)**:
- Used testnet key (original Terraform-managed version)

### ECS Service Status
```bash
# Check service
aws ecs describe-services --cluster karmacadabra-prod \
  --services karmacadabra-prod-facilitator --region us-east-1 \
  --query 'services[0].[serviceName,taskDefinition,runningCount]'

# Result: Using revision 16 (mainnet)
```

## Future Deployments

Now that the facilitator is properly managed in Terraform with the mainnet secret, you can deploy updates using standard Terraform workflow:

### Method 1: Standard Terraform Apply
```bash
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate

# Make any configuration changes to main.tf
# Then apply:
terraform plan
terraform apply
```

### Method 2: Force New Deployment (Code Changes Only)
If you just updated the Docker image and need to redeploy without Terraform changes:

```bash
# Option A: Via Terraform (triggers new deployment automatically)
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate
terraform apply -target="aws_ecs_service.agents[\"facilitator\"]"

# Option B: Via AWS CLI (quick, no Terraform state change)
aws ecs update-service --cluster karmacadabra-prod \
  --service karmacadabra-prod-facilitator \
  --force-new-deployment --region us-east-1
```

### Method 3: Update Specific Variables
To change CPU, memory, or other facilitator-specific settings:

**Edit**: `Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate\variables.tf`
```hcl
variable "facilitator_task_cpu" {
  default = 2048  # Change as needed
}

variable "facilitator_task_memory" {
  default = 4096  # Change as needed
}
```

**Apply**:
```bash
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate
terraform apply
```

## Switching Networks (Future Reference)

If you ever need to switch back to testnet or to another network:

1. **Update the secret name in main.tf** (line 62):
   ```hcl
   # For testnet:
   name = each.key == "facilitator" ? "karmacadabra-facilitator-testnet" : "karmacadabra-${each.key}"

   # For mainnet:
   name = each.key == "facilitator" ? "karmacadabra-facilitator-mainnet" : "karmacadabra-${each.key}"
   ```

2. **Apply the change**:
   ```bash
   cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate
   terraform apply
   ```

3. **Verify the new task definition**:
   ```bash
   aws ecs describe-task-definition --task-definition karmacadabra-prod-facilitator \
     --region us-east-1 --query 'taskDefinition.containerDefinitions[0].secrets'
   ```

## Important Notes

1. **State is Now Synced**: Terraform state now correctly reflects the mainnet configuration
2. **No Import Needed**: All resources were already in state; we just updated the configuration
3. **Rollback Capability**: Can easily revert to previous revision if needed
4. **Health Checks**: ECS automatically performs health checks before draining old tasks
5. **Zero Downtime**: ECS manages blue/green deployment automatically

## Rollback Procedure (If Needed)

If you need to rollback to a previous revision:

```bash
# Update the service to use an older task definition
aws ecs update-service --cluster karmacadabra-prod \
  --service karmacadabra-prod-facilitator \
  --task-definition karmacadabra-prod-facilitator:15 \
  --region us-east-1

# Then update Terraform state to match
cd Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate
terraform refresh
```

## Monitoring

### View Logs
```bash
# Real-time logs
aws logs tail /ecs/karmacadabra-prod/facilitator --follow --region us-east-1

# Last 1 hour
aws logs tail /ecs/karmacadabra-prod/facilitator --since 1h --region us-east-1
```

### Check Health
```bash
# Via domain
curl https://facilitator.karmacadabra.ultravioletadao.xyz/health

# Or via ALB
curl http://facilitator.ultravioletadao.xyz/health
```

### View Task Status
```bash
# List running tasks
aws ecs list-tasks --cluster karmacadabra-prod \
  --service-name karmacadabra-prod-facilitator \
  --desired-status RUNNING --region us-east-1

# Get task details (replace TASK_ID)
aws ecs describe-tasks --cluster karmacadabra-prod \
  --tasks TASK_ID --region us-east-1
```

## Files Modified

- `Z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate\main.tf` (line 62)

## Resources Created/Updated

- `aws_ecs_task_definition.agents["facilitator"]` - Updated to revision 16
- `aws_ecs_service.agents["facilitator"]` - Updated to use revision 16

## Completed
- Date: 2025-10-30
- Terraform State: Synced
- Deployment: In Progress (both tasks healthy, old task will drain automatically)
- Secret ARN: `arn:aws:secretsmanager:us-east-1:518898403364:secret:karmacadabra-facilitator-mainnet-WTvZkf`
