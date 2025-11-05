# Karmacadabra ECS Fargate Infrastructure

Cost-optimized AWS infrastructure for deploying the Karmacadabra AI agent system on ECS Fargate with full observability.

## Overview

This Terraform module deploys a complete, production-ready infrastructure for running 5 AI agents on AWS ECS Fargate:

- **validator** (port 9001) - Independent validation service
- **karma-hello** (port 9002) - Chat logs seller
- **abracadabra** (port 9003) - Transcription seller
- **skill-extractor** (port 9004) - Skill profiling seller
- **voice-extractor** (port 9005) - Personality profiling seller

## Cost Optimization (CRITICAL)

This module is heavily optimized for **lowest possible cost** while maintaining production-grade functionality:

### Monthly Cost Estimate: $79-96

| Component | Cost | Optimization |
|-----------|------|--------------|
| **Fargate Spot (5 agents)** | $25-40 | Using Spot pricing (70% cheaper than on-demand) |
| **Application Load Balancer** | $16-18 | Single ALB with path-based routing (vs 5 separate ALBs) |
| **NAT Gateway** | $32 | Single NAT in one AZ (vs multi-AZ HA setup) |
| **CloudWatch Logs** | $5-8 | 7-day retention only |
| **Container Insights** | Included | Essential observability |
| **ECR Image Storage** | $1-2 | Lifecycle policies to delete old images |
| **TOTAL** | **$79-96** | **Target: Under $100/month** |

### Key Cost-Saving Features

1. **Fargate Spot** - 70% cost savings vs on-demand
2. **Single NAT Gateway** - Saves ~$32/month (trade-off: no HA for NAT)
3. **Smallest Task Sizes** - 0.25 vCPU / 0.5GB RAM to start
4. **VPC Endpoints** - Reduces NAT data transfer costs
5. **Short Log Retention** - 7 days vs 30+ days
6. **Conservative Auto-Scaling** - Max 3 tasks per service
7. **No ALB Access Logs** - Saves S3 storage costs
8. **Single ALB** - Path-based routing vs separate ALBs

### Ways to Reduce Costs Further

- Scale down to 0 tasks when not in use
- Use scheduled scaling (e.g., only run during business hours)
- Reduce task CPU/memory if agents can handle it
- Disable Container Insights (saves ~$3/month, but loses visibility)
- Use S3 Gateway endpoints for additional data transfer savings

## Architecture

```
Internet
    │
    ├─── Application Load Balancer (Public Subnets)
    │    ├─── /validator/*       → Validator Target Group
    │    ├─── /karma-hello/*     → Karma-Hello Target Group
    │    ├─── /abracadabra/*     → Abracadabra Target Group
    │    ├─── /skill-extractor/* → Skill-Extractor Target Group
    │    └─── /voice-extractor/* → Voice-Extractor Target Group
    │
    └─── NAT Gateway (Single - Cost Optimized)
              │
              └─── ECS Fargate Tasks (Private Subnets)
                   ├─── Validator Service (1-3 tasks)
                   ├─── Karma-Hello Service (1-3 tasks)
                   ├─── Abracadabra Service (1-3 tasks)
                   ├─── Skill-Extractor Service (1-3 tasks)
                   └─── Voice-Extractor Service (1-3 tasks)
                        │
                        ├─── AWS Secrets Manager (credentials)
                        ├─── CloudWatch Logs (observability)
                        ├─── ECR (Docker images)
                        ├─── Service Connect (inter-agent communication)
                        └─── Blockchain RPC (Avalanche Fuji)
```

## Features

### Networking
- VPC with public and private subnets across 2 AZs
- Single NAT Gateway for cost optimization
- VPC Endpoints for ECR, S3, CloudWatch Logs, Secrets Manager
- Security groups with least-privilege access

### ECS Fargate
- ECS Cluster with Container Insights
- Task definitions with proper IAM roles
- Services with Fargate Spot capacity provider
- Auto-scaling based on CPU and memory utilization
- Service Connect for inter-agent communication

### Load Balancing
- Application Load Balancer with HTTP listener
- Path-based routing to agent services
- Health checks per service
- Target groups per agent

### Observability
- CloudWatch Logs with 7-day retention
- CloudWatch Dashboard with CPU, memory, and request metrics
- CloudWatch Alarms for high CPU, memory, and task count
- X-Ray tracing support (optional)
- Container Insights for deep metrics

### Security
- IAM roles with least-privilege policies
- Secrets Manager integration for credentials
- Private subnet deployment
- Security groups for network isolation
- ECR image scanning on push

## Prerequisites

1. **AWS Account** with appropriate permissions
2. **Terraform** >= 1.0
3. **AWS CLI** configured with credentials
4. **Docker** for building images
5. **AWS Secrets Manager** secret named `karmacadabra` with agent credentials:

```json
{
  "validator-agent": {
    "private_key": "0x...",
    "openai_api_key": "sk-proj-...",
    "address": "0x..."
  },
  "karma-hello-agent": {
    "private_key": "0x...",
    "openai_api_key": "sk-proj-...",
    "address": "0x..."
  },
  "abracadabra-agent": {
    "private_key": "0x...",
    "openai_api_key": "sk-proj-...",
    "address": "0x..."
  },
  "skill-extractor-agent": {
    "private_key": "0x...",
    "openai_api_key": "sk-proj-...",
    "address": "0x..."
  },
  "voice-extractor-agent": {
    "private_key": "0x...",
    "openai_api_key": "sk-proj-...",
    "address": "0x..."
  }
}
```

## Quick Start

### 1. Configure Variables

```bash
cd terraform/ecs-fargate
cp terraform.tfvars.example terraform.tfvars
# Edit terraform.tfvars with your configuration
```

### 2. Initialize Terraform

```bash
terraform init
```

### 3. Plan Deployment

```bash
terraform plan -out=tfplan
```

Review the plan carefully, especially:
- Estimated costs
- Resource counts
- Security group rules

### 4. Apply Infrastructure

```bash
terraform apply tfplan
```

This will create:
- VPC with subnets, NAT gateway, VPC endpoints
- ECS cluster
- ECR repositories
- Application Load Balancer
- CloudWatch log groups and dashboard
- IAM roles and policies
- Security groups

### 5. Build and Push Docker Images

```bash
# Get the ECR login command from Terraform output
terraform output -raw deployment_commands

# Or manually:
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin <account-id>.dkr.ecr.us-east-1.amazonaws.com

# Build and push each agent
cd /home/user/karmacadabra

# Validator
docker build -f Dockerfile.agent -t karmacadabra/validator .
docker tag karmacadabra/validator:latest <ecr-repo-url>/karmacadabra/validator:latest
docker push <ecr-repo-url>/karmacadabra/validator:latest

# Karma-Hello
docker build -f Dockerfile.agent -t karmacadabra/karma-hello .
docker tag karmacadabra/karma-hello:latest <ecr-repo-url>/karmacadabra/karma-hello:latest
docker push <ecr-repo-url>/karmacadabra/karma-hello:latest

# Repeat for abracadabra, skill-extractor, voice-extractor
```

### 6. Update ECS Services

After pushing images, force ECS to deploy new tasks:

```bash
aws ecs update-service \
  --cluster karmacadabra-prod \
  --service karmacadabra-prod-validator \
  --force-new-deployment

# Repeat for each service
```

### 7. Verify Deployment

```bash
# Get ALB DNS name
ALB_DNS=$(terraform output -raw alb_dns_name)

# Test health checks
curl http://$ALB_DNS/validator/health
curl http://$ALB_DNS/karma-hello/health
curl http://$ALB_DNS/abracadabra/health
curl http://$ALB_DNS/skill-extractor/health
curl http://$ALB_DNS/voice-extractor/health
```

## Operations

### View Logs

```bash
# Tail logs for a service
aws logs tail /ecs/karmacadabra-prod/validator --follow

# View last 100 lines
aws logs tail /ecs/karmacadabra-prod/validator --since 1h
```

### Debug Tasks (ECS Exec)

```bash
# Get task ID
TASK_ID=$(aws ecs list-tasks \
  --cluster karmacadabra-prod \
  --service-name karmacadabra-prod-validator \
  --query 'taskArns[0]' \
  --output text | cut -d'/' -f3)

# SSH into container
aws ecs execute-command \
  --cluster karmacadabra-prod \
  --task $TASK_ID \
  --container validator \
  --interactive \
  --command '/bin/bash'
```

### Scale Services

```bash
# Manual scaling
aws ecs update-service \
  --cluster karmacadabra-prod \
  --service karmacadabra-prod-validator \
  --desired-count 3

# Auto-scaling is enabled by default (1-3 tasks)
# Triggers: CPU > 75%, Memory > 80%
```

### Update Task Definitions

```bash
# Make changes to task definition in Terraform
# Then apply:
terraform apply

# Force new deployment
aws ecs update-service \
  --cluster karmacadabra-prod \
  --service karmacadabra-prod-validator \
  --force-new-deployment
```

### Monitor Costs

```bash
# View CloudWatch Dashboard
terraform output cloudwatch_dashboard_url

# Check AWS Cost Explorer
# Services to monitor: ECS, EC2 (Fargate), CloudWatch, ECR, NAT Gateway
```

## Monitoring & Observability

### CloudWatch Dashboard

Access the pre-configured dashboard:

```bash
terraform output cloudwatch_dashboard_url
```

The dashboard includes:
- CPU utilization per agent
- Memory utilization per agent
- ALB request count per agent
- Task count per service

### CloudWatch Alarms

Alarms are configured for:
- **High CPU** (> 85%) - triggers scale-up
- **High Memory** (> 85%) - triggers scale-up
- **Low Task Count** (< 1) - service health issue
- **Unhealthy Targets** - ALB health check failures

### CloudWatch Logs Insights Queries

```sql
-- Find errors in last hour
fields @timestamp, @message
| filter @message like /ERROR/
| sort @timestamp desc
| limit 100

-- Count requests by agent
fields @timestamp
| stats count() by bin(5m)

-- Average response time
fields @timestamp, response_time
| stats avg(response_time) by bin(5m)
```

### X-Ray Tracing

X-Ray is configured but requires agent instrumentation. Add to your Python code:

```python
from aws_xray_sdk.core import xray_recorder
from aws_xray_sdk.ext.flask.middleware import XRayMiddleware

app = FastAPI()
XRayMiddleware(app, xray_recorder)
```

## Cost Management

### Current Spend Analysis

```bash
# Get detailed cost breakdown
aws ce get-cost-and-usage \
  --time-period Start=2025-10-01,End=2025-10-31 \
  --granularity MONTHLY \
  --metrics BlendedCost \
  --group-by Type=SERVICE

# Filter by project tag
aws ce get-cost-and-usage \
  --time-period Start=2025-10-01,End=2025-10-31 \
  --granularity MONTHLY \
  --metrics BlendedCost \
  --filter file://cost-filter.json
```

### Cost Optimization Actions

#### 1. Scale Down During Off-Hours

```bash
# Create scheduled scaling (9 AM - 6 PM weekdays only)
aws application-autoscaling put-scheduled-action \
  --service-namespace ecs \
  --scalable-dimension ecs:service:DesiredCount \
  --resource-id service/karmacadabra-prod/karmacadabra-prod-validator \
  --scheduled-action-name scale-down-evening \
  --schedule "cron(0 18 * * MON-FRI *)" \
  --scalable-target-action MinCapacity=0,MaxCapacity=0

aws application-autoscaling put-scheduled-action \
  --service-namespace ecs \
  --scalable-dimension ecs:service:DesiredCount \
  --resource-id service/karmacadabra-prod/karmacadabra-prod-validator \
  --scheduled-action-name scale-up-morning \
  --schedule "cron(0 9 * * MON-FRI *)" \
  --scalable-target-action MinCapacity=1,MaxCapacity=3
```

#### 2. Use Smaller Task Sizes

```hcl
# In terraform.tfvars
task_cpu    = 256  # 0.25 vCPU (minimum)
task_memory = 512  # 0.5 GB (minimum)

# If agents can handle less memory:
# task_memory = 512  # Test with minimum first
```

#### 3. Reduce Log Retention

```hcl
# In terraform.tfvars
log_retention_days = 3  # Instead of 7
```

#### 4. Disable Container Insights (if not needed)

```hcl
# In terraform.tfvars
enable_container_insights = false  # Saves ~$3/month
```

### Cost Monitoring Alerts

Set up billing alerts in AWS Budgets:

```bash
aws budgets create-budget \
  --account-id $(aws sts get-caller-identity --query Account --output text) \
  --budget file://budget.json \
  --notifications-with-subscribers file://notifications.json
```

## Troubleshooting

### Issue: Tasks Not Starting

**Symptoms**: Tasks stuck in PENDING state

**Solutions**:

1. Check CloudWatch Logs for errors:
   ```bash
   aws logs tail /ecs/karmacadabra-prod/validator --follow
   ```

2. Verify IAM role permissions:
   ```bash
   aws iam get-role --role-name karmacadabra-prod-ecs-task-execution
   ```

3. Check Secrets Manager access:
   ```bash
   aws secretsmanager get-secret-value --secret-id karmacadabra
   ```

4. Verify ECR image exists:
   ```bash
   aws ecr describe-images --repository-name karmacadabra/validator
   ```

### Issue: High Costs

**Symptoms**: Monthly bill exceeds estimate

**Solutions**:

1. Check Fargate Spot usage:
   ```bash
   aws ecs describe-services --cluster karmacadabra-prod --services karmacadabra-prod-validator
   # Look for capacityProviderStrategy
   ```

2. Verify NAT Gateway data transfer:
   ```bash
   # Check CloudWatch metric: NATGateway -> BytesOutToSource
   ```

3. Check CloudWatch Logs ingestion:
   ```bash
   aws logs describe-log-groups --log-group-name-prefix /ecs/karmacadabra
   ```

### Issue: Services Not Responding

**Symptoms**: Health checks failing, 502/503 errors

**Solutions**:

1. Check target health:
   ```bash
   aws elbv2 describe-target-health --target-group-arn <arn>
   ```

2. Verify security groups:
   ```bash
   # Ensure ECS tasks SG allows traffic from ALB SG on agent ports
   aws ec2 describe-security-groups --group-ids <ecs-tasks-sg-id>
   ```

3. Check container logs:
   ```bash
   aws logs tail /ecs/karmacadabra-prod/validator --follow
   ```

4. Verify environment variables:
   ```bash
   aws ecs describe-task-definition --task-definition karmacadabra-prod-validator
   ```

### Issue: Inter-Agent Communication Failing

**Symptoms**: Agents can't call each other (e.g., skill-extractor → karma-hello)

**Solutions**:

1. Verify Service Connect:
   ```bash
   aws ecs describe-services --cluster karmacadabra-prod --services karmacadabra-prod-skill-extractor
   # Check serviceConnectConfiguration
   ```

2. Check DNS resolution:
   ```bash
   # From inside container:
   nslookup karma-hello.karmacadabra.local
   ```

3. Verify security group rules:
   ```bash
   # ECS tasks SG must allow ingress from itself
   aws ec2 describe-security-groups --group-ids <ecs-tasks-sg-id>
   ```

## File Structure

```
terraform/ecs-fargate/
├── README.md                    # This file
├── main.tf                      # ECS cluster, services, task definitions
├── vpc.tf                       # VPC, subnets, NAT, VPC endpoints
├── alb.tf                       # Application Load Balancer
├── iam.tf                       # IAM roles and policies
├── security_groups.tf           # Security groups
├── cloudwatch.tf                # Logs, metrics, alarms, dashboard
├── ecr.tf                       # ECR repositories
├── variables.tf                 # Input variables
├── outputs.tf                   # Output values
└── terraform.tfvars.example     # Example configuration
```

## Terraform Resources

This module creates approximately:
- 1 VPC
- 4 Subnets (2 public, 2 private)
- 1 Internet Gateway
- 1 NAT Gateway (or 2 for HA)
- 5+ VPC Endpoints
- 1 ECS Cluster
- 5 ECS Services
- 5 ECS Task Definitions
- 1 Application Load Balancer
- 5 Target Groups
- 5 ECR Repositories
- 5 CloudWatch Log Groups
- 1 CloudWatch Dashboard
- 15+ CloudWatch Alarms
- 10+ Security Group Rules
- 5+ IAM Roles
- 10+ IAM Policies

**Total**: ~100+ resources

## Security Best Practices

1. **Secrets Management**: All credentials in AWS Secrets Manager
2. **Least Privilege IAM**: Minimal permissions per role
3. **Private Subnets**: Tasks run in private subnets only
4. **Security Groups**: Restrict traffic to necessary ports only
5. **ECR Scanning**: Automatic image vulnerability scanning
6. **No Public IPs**: Tasks use NAT for outbound only
7. **VPC Endpoints**: Private connections to AWS services
8. **Encryption**: Secrets encrypted at rest (KMS)

## Maintenance

### Regular Tasks

- **Weekly**: Review CloudWatch Dashboard for anomalies
- **Monthly**: Check AWS Cost Explorer for spend trends
- **Monthly**: Review and delete old ECR images (automated)
- **Quarterly**: Review and update task definitions
- **Quarterly**: Update Terraform providers and modules

### Upgrades

```bash
# Update Terraform providers
terraform init -upgrade

# Apply updates
terraform plan
terraform apply

# Force new deployments with updated images
for service in validator karma-hello abracadabra skill-extractor voice-extractor; do
  aws ecs update-service \
    --cluster karmacadabra-prod \
    --service karmacadabra-prod-$service \
    --force-new-deployment
done
```

## Cleanup

To destroy all infrastructure:

```bash
# WARNING: This will delete EVERYTHING
terraform destroy

# Or selectively destroy:
terraform destroy -target=aws_ecs_service.agents
```

## Support

For issues or questions:
1. Check CloudWatch Logs first
2. Review this README troubleshooting section
3. Check Terraform state: `terraform show`
4. Review AWS Console ECS/CloudWatch sections

## License

Same as the Karmacadabra project.

## Related Documentation

- [Karmacadabra Architecture](../../docs/ARCHITECTURE.md)
- [Docker Guide](../../docs/guides/DOCKER_GUIDE.md)
- [Master Plan](../../MASTER_PLAN.md)
- [AWS ECS Documentation](https://docs.aws.amazon.com/ecs/)
- [Terraform AWS Provider](https://registry.terraform.io/providers/hashicorp/aws/latest/docs)
