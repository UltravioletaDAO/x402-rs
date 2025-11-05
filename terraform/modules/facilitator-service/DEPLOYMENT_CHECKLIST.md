# ECS Fargate Deployment Checklist

Complete checklist for deploying Karmacadabra agents to AWS ECS Fargate.

## Pre-Deployment

### AWS Account Setup

- [ ] AWS account created and configured
- [ ] AWS CLI installed and configured (`aws configure`)
- [ ] IAM user with appropriate permissions:
  - [ ] ECS full access
  - [ ] ECR full access
  - [ ] VPC full access
  - [ ] IAM role creation
  - [ ] CloudWatch full access
  - [ ] Secrets Manager read access
  - [ ] Application Load Balancer full access

### Prerequisites

- [ ] Terraform >= 1.0 installed (`terraform --version`)
- [ ] Docker installed and running (`docker --version`)
- [ ] jq installed for JSON parsing (`jq --version`)
- [ ] Git repository cloned
- [ ] AWS region selected (default: us-east-2)

### Secrets Manager Setup

- [ ] AWS Secrets Manager secret created named `karmacadabra`
- [ ] Secret contains all agent credentials:
  ```json
  {
    "validator-agent": {
      "private_key": "0x...",
      "openai_api_key": "sk-proj-...",
      "address": "0x..."
    },
    "karma-hello-agent": { ... },
    "abracadabra-agent": { ... },
    "skill-extractor-agent": { ... },
    "voice-extractor-agent": { ... }
  }
  ```
- [ ] Secret accessible from target AWS region

### Configuration

- [ ] Copy `terraform.tfvars.example` to `terraform.tfvars`
- [ ] Review and update configuration:
  - [ ] `project_name` set correctly
  - [ ] `environment` set (dev/staging/prod)
  - [ ] `aws_region` set to desired region
  - [ ] `use_fargate_spot = true` (CRITICAL for cost savings)
  - [ ] `single_nat_gateway = true` (CRITICAL for cost savings)
  - [ ] `task_cpu` and `task_memory` set appropriately
  - [ ] `secrets_manager_secret_name` matches your secret
  - [ ] Tags configured with owner/cost center info

## Deployment

### Step 1: Initialize Terraform

```bash
cd terraform/ecs-fargate
make init
# or
terraform init
```

- [ ] Terraform initialized successfully
- [ ] Provider plugins downloaded
- [ ] No errors in output

### Step 2: Plan Infrastructure

```bash
make plan
# or
terraform plan -out=tfplan
```

- [ ] Plan executed successfully
- [ ] Reviewed all resources to be created (~100+ resources)
- [ ] Verified cost estimates are acceptable
- [ ] No unexpected changes
- [ ] Saved plan to `tfplan`

### Step 3: Apply Infrastructure

```bash
make apply
# or
terraform apply tfplan
```

- [ ] Apply completed successfully
- [ ] VPC created
- [ ] ECS cluster created
- [ ] ECR repositories created
- [ ] Load balancer created
- [ ] Security groups configured
- [ ] CloudWatch log groups created
- [ ] No errors in output
- [ ] Outputs displayed correctly

### Step 4: Build and Push Docker Images

```bash
make push-images
# or manually for each agent:
make ecr-login
make build-validator
make push-validator
# Repeat for each agent
```

- [ ] ECR login successful
- [ ] Validator image built and pushed
- [ ] Karma-hello image built and pushed
- [ ] Abracadabra image built and pushed
- [ ] Skill-extractor image built and pushed
- [ ] Voice-extractor image built and pushed
- [ ] All images visible in ECR console

### Step 5: Deploy Services

```bash
make update-services
# or manually:
aws ecs update-service --cluster facilitator-production --service facilitator-production --force-new-deployment --region us-east-2
```

- [ ] All services updated
- [ ] Tasks starting (check ECS console)
- [ ] No immediate failures

## Post-Deployment Verification

### Health Checks

```bash
make health-check
# or manually:
ALB_DNS=$(terraform output -raw alb_dns_name)
curl http://$ALB_DNS/validator/health
```

- [ ] Validator health check returns 200
- [ ] Karma-hello health check returns 200
- [ ] Abracadabra health check returns 200
- [ ] Skill-extractor health check returns 200
- [ ] Voice-extractor health check returns 200

### ECS Console Verification

- [ ] Navigate to ECS console
- [ ] Cluster shows "Active"
- [ ] All 5 services running
- [ ] All services show "Running" tasks
- [ ] No tasks in "Stopped" state with errors
- [ ] Task count matches desired count (default: 1 per service)

### CloudWatch Logs

```bash
make logs-validator
# Check logs for each service
```

- [ ] Logs streaming to CloudWatch
- [ ] No critical errors in startup logs
- [ ] Agents successfully fetched secrets from AWS Secrets Manager
- [ ] Agents connected to blockchain RPC
- [ ] Agents registered on-chain (if applicable)

### Load Balancer Verification

- [ ] Navigate to EC2 > Load Balancers
- [ ] ALB shows "Active" state
- [ ] All target groups have healthy targets
- [ ] No targets in "Unhealthy" state
- [ ] Access ALB DNS name in browser

### Auto-Scaling

- [ ] Navigate to ECS > Clusters > Services
- [ ] Auto-scaling policies visible
- [ ] Target tracking scaling enabled
- [ ] Min capacity: 1, Max capacity: 3

### CloudWatch Dashboard

```bash
make dashboard
# or
terraform output cloudwatch_dashboard_url
```

- [ ] Dashboard opens successfully
- [ ] CPU metrics visible for all agents
- [ ] Memory metrics visible for all agents
- [ ] Request count showing activity
- [ ] No missing data points

### CloudWatch Alarms

- [ ] Navigate to CloudWatch > Alarms
- [ ] High CPU alarms created (5 total)
- [ ] High Memory alarms created (5 total)
- [ ] Low task count alarms created (5 total)
- [ ] Unhealthy target alarms created (5 total)
- [ ] All alarms in "OK" state (or "INSUFFICIENT_DATA" initially)

## Cost Verification

### Cost Explorer

- [ ] Navigate to AWS Cost Explorer
- [ ] Enable Cost Explorer (if first time)
- [ ] Filter by tags: Project=Karmacadabra
- [ ] Verify daily spend is within expected range
- [ ] Set up budget alerts if needed

### Expected Costs (Monthly)

- [ ] Fargate Spot (5 agents, 24/7): $25-40
- [ ] Application Load Balancer: $16-18
- [ ] NAT Gateway (single): ~$32
- [ ] CloudWatch Logs (7-day retention): $5-8
- [ ] Container Insights: ~$3
- [ ] ECR image storage: $1-2
- [ ] **Total expected: $79-96/month**

### Cost Optimization Verification

- [ ] Fargate Spot enabled (check capacity provider strategy)
- [ ] Single NAT Gateway confirmed (not multi-AZ)
- [ ] Log retention set to 7 days
- [ ] No unnecessary resources running
- [ ] Auto-scaling max set to 3 (not higher)

## Functional Testing

### Basic API Tests

```bash
ALB_DNS=$(terraform output -raw alb_dns_name)

# Test each agent's API
curl http://$ALB_DNS/validator/health
curl http://$ALB_DNS/karma-hello/health
curl http://$ALB_DNS/abracadabra/health
curl http://$ALB_DNS/skill-extractor/health
curl http://$ALB_DNS/voice-extractor/health
```

- [ ] All health endpoints return 200 OK
- [ ] Response times reasonable (<1 second)

### Inter-Agent Communication

- [ ] Service Connect enabled (check ECS service config)
- [ ] Agents can communicate internally (check logs)
- [ ] Example: skill-extractor can call karma-hello

### Blockchain Integration

- [ ] Agents can reach Avalanche Fuji RPC
- [ ] Agents registered on-chain (check Identity Registry)
- [ ] Transactions being signed successfully
- [ ] No blockchain connection errors in logs

### AI/OpenAI Integration

- [ ] OpenAI API keys loaded from Secrets Manager
- [ ] CrewAI agents functioning
- [ ] No API key errors in logs
- [ ] LLM responses being generated

## Monitoring Setup

### CloudWatch Alarms

- [ ] SNS topic created (if using notifications)
- [ ] SNS topic subscribed to email/SMS
- [ ] Test alarm by triggering high CPU (optional)
- [ ] Verify alarm notifications received

### Log Insights Queries

- [ ] Save useful queries in CloudWatch Logs Insights:
  - [ ] Error count query
  - [ ] Request latency query
  - [ ] Agent-specific queries

### Dashboard Customization

- [ ] Review default dashboard
- [ ] Add custom widgets if needed
- [ ] Set dashboard as favorite
- [ ] Share dashboard URL with team

## Documentation

- [ ] Update internal documentation with:
  - [ ] ALB DNS name
  - [ ] ECR repository URLs
  - [ ] CloudWatch dashboard URL
  - [ ] Log group names
  - [ ] Runbook for common operations

## Backup & Disaster Recovery

### Terraform State

- [ ] Terraform state backed up
- [ ] Consider migrating to S3 backend (see main.tf comments)
- [ ] State locking with DynamoDB (if using S3 backend)

### Configuration Backup

- [ ] `terraform.tfvars` backed up securely (not in git)
- [ ] Secrets documented (securely, not in git)
- [ ] Deployment scripts backed up

### Disaster Recovery Plan

- [ ] Documented procedure to recreate infrastructure
- [ ] Secrets stored in multiple locations (AWS Secrets Manager + secure vault)
- [ ] Docker images tagged and stored long-term

## Security Review

### Network Security

- [ ] Tasks in private subnets only
- [ ] No public IPs assigned to tasks
- [ ] Security groups properly configured
- [ ] ALB security group allows only 80/443
- [ ] ECS tasks security group allows only necessary ports

### IAM Security

- [ ] Task execution role has minimal permissions
- [ ] Task role has minimal permissions
- [ ] No overly permissive policies
- [ ] Secrets access restricted to specific secret ARN

### Secrets Security

- [ ] Secrets Manager secret encrypted
- [ ] No secrets in Terraform code
- [ ] No secrets in Docker images
- [ ] Secrets rotation plan in place

### Image Security

- [ ] ECR image scanning enabled
- [ ] No critical vulnerabilities in images
- [ ] Images from trusted base (python:3.11-slim)
- [ ] Regular image updates planned

## Operational Readiness

### Runbooks Created

- [ ] How to deploy new version
- [ ] How to roll back deployment
- [ ] How to scale services
- [ ] How to investigate errors
- [ ] How to access container logs
- [ ] How to SSH into containers (ECS Exec)

### Team Training

- [ ] Team familiar with ECS console
- [ ] Team familiar with CloudWatch
- [ ] Team knows how to view logs
- [ ] Team knows how to trigger deployments
- [ ] Team knows cost optimization strategies

### On-Call Setup

- [ ] On-call rotation defined
- [ ] Escalation procedures documented
- [ ] Access credentials distributed
- [ ] Runbooks accessible to on-call

## Performance Baseline

### Metrics to Track

- [ ] Average CPU utilization per service
- [ ] Average memory utilization per service
- [ ] Request count per service
- [ ] Response time per service
- [ ] Error rate per service

### Baselines Established

- [ ] Normal CPU range: ___% - ___%
- [ ] Normal memory range: ___% - ___%
- [ ] Normal request latency: ___ ms
- [ ] Acceptable error rate: < ___%

## Optimization Opportunities

### Identified for Future

- [ ] Task size optimization (test with smaller CPU/memory)
- [ ] Scheduled scaling (scale down off-hours)
- [ ] Reserved capacity (if usage predictable)
- [ ] Multi-region deployment (future)
- [ ] CDN for static assets (if applicable)

## Sign-Off

- [ ] Infrastructure deployed successfully
- [ ] All services healthy and running
- [ ] Monitoring and alerting configured
- [ ] Documentation complete
- [ ] Team trained and ready
- [ ] Costs within expected range

**Deployment Date**: _______________

**Deployed By**: _______________

**Reviewed By**: _______________

**Production Go-Live Date**: _______________

## Troubleshooting Reference

Common issues and solutions:

1. **Tasks not starting**: Check CloudWatch logs, verify Secrets Manager access
2. **Health checks failing**: Verify security groups, check container logs
3. **High costs**: Verify Fargate Spot enabled, check NAT data transfer
4. **Inter-agent communication failing**: Verify Service Connect, check security groups
5. **Secrets not loading**: Verify IAM permissions, check secret name

For detailed troubleshooting, see [README.md](./README.md#troubleshooting).
