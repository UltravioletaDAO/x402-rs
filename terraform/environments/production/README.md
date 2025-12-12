# Facilitator Production Infrastructure

This directory contains Terraform configuration for the Facilitator production environment deployed on AWS ECS in `us-east-2`.

## Quick Start

```bash
# 1. Validate secrets exist
bash validate_secrets.sh us-east-2

# 2. Initialize Terraform
terraform init

# 3. Review changes
terraform plan -out=facilitator.tfplan

# 4. Apply changes
terraform apply facilitator.tfplan

# 5. Force ECS deployment to pick up new configuration
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

## File Organization

```
terraform/environments/production/
├── README.md                    # This file
├── SECRETS_MANAGEMENT.md        # Complete secrets documentation
├── DEPLOYMENT_CHECKLIST.md      # Step-by-step deployment guide
├── validate_secrets.sh          # Pre-deployment validation script
│
├── backend.tf                   # S3 backend configuration
├── variables.tf                 # Input variables
├── outputs.tf                   # Output values
│
├── secrets.tf                   # SINGLE SOURCE OF TRUTH for secrets
├── main.tf                      # Primary infrastructure (VPC, ALB, ECS)
│
├── cloudwatch-near-metrics.tf   # NEAR-specific CloudWatch metrics
└── cloudwatch-v2-metrics.tf     # V2 metrics and alarms
```

## Architecture Overview

**Infrastructure Stack:**
- **VPC:** 10.1.0.0/16 with public/private subnets in 2 AZs
- **ALB:** Application Load Balancer with HTTPS termination
- **ECS Fargate:** 1 vCPU, 2GB RAM, auto-scaling 1-3 tasks
- **NAT Gateway:** For private subnet internet access
- **Route53:** DNS for facilitator.ultravioletadao.xyz
- **Secrets Manager:** Wallet keys and premium RPC URLs
- **CloudWatch:** Logs and metrics

**Cost:** ~$43-48/month
- ALB: ~$17/month
- NAT Gateway: ~$32/month (single NAT for cost optimization)
- Fargate: ~$15/month (1 vCPU, 2GB RAM, 1 task)
- Secrets Manager: ~$5/month (15 secrets)
- CloudWatch Logs: ~$1/month (7-day retention)

## Supported Networks

**26 Networks Total:**

**EVM Networks (18):**
- Mainnets: base, avalanche, polygon, optimism, celo, hyperevm, ethereum, arbitrum, unichain
- Testnets: base-sepolia, avalanche-fuji, polygon-amoy, optimism-sepolia, celo-sepolia, hyperevm-testnet, ethereum-sepolia, arbitrum-sepolia, unichain-sepolia

**Solana/SVM Networks (4):**
- Mainnets: solana, fogo
- Testnets: solana-devnet, fogo-testnet

**NEAR Networks (2):**
- near, near-testnet

**Stellar Networks (2):**
- stellar, stellar-testnet

## Secrets Architecture

All secrets are defined in `secrets.tf` as the SINGLE SOURCE OF TRUTH.

**Why this matters:**
- Before: Secrets were manually added to task definitions, easy to forget
- After: All secrets in one file, Terraform validates they exist, impossible to deploy without them
- See `SECRETS_MANAGEMENT.md` for complete documentation

**Secret Categories:**

1. **Wallet Secrets** (10 total):
   - EVM: mainnet, testnet, legacy
   - Solana: mainnet, testnet, legacy
   - NEAR: mainnet, testnet
   - Stellar: mainnet, testnet

2. **RPC URL Secrets** (2 total):
   - facilitator-rpc-mainnet (12 networks)
   - facilitator-rpc-testnet (3 networks)

**Adding a new network:**
1. Add wallet secret to AWS Secrets Manager (if new chain family)
2. Add RPC URL to appropriate secret (mainnet or testnet)
3. Update `secrets.tf` with new references
4. Run `terraform plan` to verify
5. Deploy with `terraform apply`

See `SECRETS_MANAGEMENT.md` for detailed instructions.

## Deployment Process

**Pre-Deployment:**
1. Validate secrets: `bash validate_secrets.sh us-east-2`
2. Update image tag in `variables.tf`
3. Review code changes: `git log --oneline -10`

**Deployment:**
1. `terraform init` (if first time or after module changes)
2. `terraform validate` (check syntax)
3. `terraform plan -out=facilitator.tfplan` (review changes)
4. `terraform apply facilitator.tfplan` (apply changes)
5. `aws ecs update-service --force-new-deployment` (restart tasks)

**Post-Deployment:**
1. Monitor deployment: `aws ecs describe-services`
2. Check logs: `aws logs tail /ecs/facilitator-production --follow`
3. Verify health: `curl https://facilitator.ultravioletadao.xyz/health`
4. Test networks: `curl https://facilitator.ultravioletadao.xyz/supported`
5. Run integration tests: `cd tests/integration && python test_facilitator.py`

See `DEPLOYMENT_CHECKLIST.md` for complete step-by-step guide.

## State Management

**Backend:** S3 + DynamoDB
- Bucket: `facilitator-terraform-state`
- Key: `production/terraform.tfstate`
- Region: `us-east-2`
- Locking: DynamoDB table `facilitator-terraform-locks`

**Locking ensures:**
- Only one person can apply changes at a time
- Prevents state corruption
- Provides audit trail of changes

**State commands:**
```bash
# List resources
terraform state list

# Show specific resource
terraform state show aws_ecs_service.facilitator

# Pull latest state
terraform state pull > /tmp/state.json

# Refresh state from AWS
terraform refresh
```

## Variables

**Key Variables:**
```hcl
# Image configuration
variable "image_tag" {
  default = "v1.3.6"  # Update before deployment
}

# Resource sizing
variable "task_cpu" {
  default = 1024  # 1 vCPU
}

variable "task_memory" {
  default = 2048  # 2GB RAM
}

# Auto-scaling
variable "min_capacity" {
  default = 1  # Minimum tasks
}

variable "max_capacity" {
  default = 3  # Maximum tasks
}

# Domain
variable "domain_name" {
  default = "facilitator.ultravioletadao.xyz"
}
```

See `variables.tf` for full list.

## Outputs

After applying, Terraform outputs useful values:

```bash
# Get all outputs
terraform output

# Specific outputs
terraform output alb_dns_name
terraform output ecs_cluster_name
terraform output task_definition_arn
```

## Monitoring and Logging

**CloudWatch Logs:**
```bash
# Stream logs
aws logs tail /ecs/facilitator-production --follow --region us-east-2

# Search for errors
aws logs filter-log-events \
  --log-group-name /ecs/facilitator-production \
  --filter-pattern "ERROR" \
  --region us-east-2
```

**CloudWatch Metrics:**
- ECS service metrics (CPU, memory, task count)
- NEAR-specific metrics (balance, transaction count)
- Custom application metrics (payment volume, error rate)

**Alarms:**
- High CPU utilization (>75% for 5 minutes)
- High memory utilization (>80% for 5 minutes)
- Task count = 0 (service down)
- NEAR wallet balance low (<5 NEAR)

## Auto-Scaling

**Scaling Policies:**
- CPU-based: Target 75% utilization
- Memory-based: Target 80% utilization

**Behavior:**
- Scale up: Add task when utilization exceeds target for 3 minutes
- Scale down: Remove task when utilization below target for 10 minutes
- Min tasks: 1 (always at least one running)
- Max tasks: 3 (cost constraint)

## Security

**IAM Roles:**
- **Execution Role:** Can pull secrets and Docker images (used during startup)
- **Task Role:** Application runtime permissions (currently none needed)

**Secrets Manager:**
- All sensitive values (wallet keys, RPC URLs with API keys)
- Never in environment variables or task definition
- Encrypted at rest with KMS
- Access logged in CloudTrail

**Network Security:**
- Private subnets for ECS tasks (no direct internet access)
- NAT Gateway for outbound traffic (RPC calls)
- ALB in public subnets (ingress only on port 443)
- Security groups limit traffic (ALB -> Tasks on port 8080)

**HTTPS:**
- ACM certificate for facilitator.ultravioletadao.xyz
- TLS 1.3 enforced
- HTTP redirects to HTTPS

## Troubleshooting

### Common Issues

**1. "ResourceInitializationError" when starting task**

Cause: Missing IAM permissions for secrets or ECR.

Solution:
```bash
bash validate_secrets.sh us-east-2
terraform apply -auto-approve  # Fixes IAM policy
```

**2. Task starts but health check fails**

Cause: Application error on startup.

Solution:
```bash
aws logs tail /ecs/facilitator-production --follow --region us-east-2
# Check logs for error messages
```

**3. "No healthy targets" in ALB**

Cause: Tasks failing health check.

Solution:
```bash
# Check task logs
aws logs tail /ecs/facilitator-production --follow --region us-east-2

# Verify health check endpoint
curl http://TASK_PRIVATE_IP:8080/health
```

**4. Network not supported after adding**

Cause: Missing secret reference in task definition.

Solution:
```bash
# Verify secret in secrets.tf
grep -A 5 "YOUR_NETWORK" secrets.tf

# Re-apply Terraform
terraform apply -auto-approve

# Force deployment
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

### Rollback

**Quick rollback to previous task definition:**
```bash
# List recent task definitions
aws ecs list-task-definitions --family-prefix facilitator-production \
  --sort DESC --max-items 5 --region us-east-2

# Update service to previous revision
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:PREVIOUS_REVISION \
  --force-new-deployment --region us-east-2
```

**Full Terraform rollback:**
```bash
git checkout HEAD~1 -- variables.tf
terraform apply -auto-approve
aws ecs update-service --cluster facilitator-production \
  --service facilitator-production --force-new-deployment --region us-east-2
```

## Cost Optimization

Current configuration is optimized for ~$45/month budget:

**Optimization Strategies:**
- Single NAT Gateway (not one per AZ) → saves $32/month
- No VPC Endpoints → saves $35/month (uses NAT for AWS API calls)
- Fargate Spot NOT used → reliability over cost savings
- 7-day log retention → minimal storage costs
- Single task by default → scales up only when needed

**Potential Optimizations:**
- Switch to NAT Instance → saves $25/month (but less reliable)
- Use VPC Endpoints + remove NAT → saves $22/month (but complex setup)
- Reduce log retention to 1 day → saves $0.50/month (not recommended)

## Disaster Recovery

**Backup Strategy:**
- Terraform state in S3 with versioning enabled
- Daily snapshots of secrets (manual process)
- Git repository contains all infrastructure code

**Recovery Procedure:**
1. Restore Terraform state from S3 version history
2. Re-apply infrastructure: `terraform apply`
3. Restore secrets from backup
4. Force ECS deployment

**RTO (Recovery Time Objective):** 30 minutes
**RPO (Recovery Point Objective):** 1 day (manual secret backups)

## Compliance

**Audit Trail:**
- Terraform state changes logged in S3 versioning
- AWS API calls logged in CloudTrail
- Secret access logged in CloudTrail
- Git commits for infrastructure changes

**Security Standards:**
- Encryption at rest (S3, Secrets Manager, ECS)
- Encryption in transit (TLS 1.3)
- Least privilege IAM policies
- Network segmentation (public/private subnets)

## Additional Resources

- [AWS ECS Best Practices](https://docs.aws.amazon.com/AmazonECS/latest/bestpracticesguide/)
- [Terraform AWS Provider](https://registry.terraform.io/providers/hashicorp/aws/latest/docs)
- [Secrets Management Guide](SECRETS_MANAGEMENT.md)
- [Deployment Checklist](DEPLOYMENT_CHECKLIST.md)
- [Wallet Rotation Procedures](/mnt/z/ultravioleta/dao/x402-rs/docs/WALLET_ROTATION.md)

## Support

For infrastructure issues:
1. Check this README and related docs
2. Review CloudWatch logs and metrics
3. Check Terraform state: `terraform state list`
4. Contact infrastructure lead
5. Open AWS Support case (if critical)

For application issues:
1. Check application logs in CloudWatch
2. Review recent code changes
3. Run integration tests
4. Contact development team
