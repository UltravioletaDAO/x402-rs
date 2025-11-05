# AWS Facilitator Infrastructure Extraction Plan

**Analysis Date**: 2025-11-01
**Target**: Extract x402-rs facilitator to standalone repository with independent AWS deployment
**Current State**: Shared ECS cluster, shared infrastructure with 6+ agents

---

## Executive Summary

The x402-rs facilitator is currently deployed as part of a multi-agent ECS Fargate cluster (`karmacadabra-prod`) with shared networking, load balancing, and DNS infrastructure. To extract it into a standalone repository capable of independent deployment, we must:

1. **Preserve shared infrastructure** (VPC, ALB, Route53) while allowing optional independent deployment
2. **Extract facilitator-specific resources** (task definition, secrets, ECR repository)
3. **Create portable Terraform modules** that work both standalone and as part of larger infrastructure
4. **Maintain zero-downtime migration** capability with blue-green deployment strategy

**Key Decision**: Use **Terraform module extraction** approach - facilitator becomes a reusable module that can be deployed standalone OR imported into parent infrastructure.

---

## Current Infrastructure Inventory

### 1. ECS Cluster & Service Configuration

**Cluster**: `karmacadabra-prod` (SHARED)
- Region: `us-east-1`
- Container Insights: Enabled
- Capacity Providers: `FARGATE` (100% weight for facilitator, on-demand)
- Cost: ~$0 (cluster itself), facilitator service ~$40-50/month

**Service**: `karmacadabra-prod-facilitator`
- Desired Count: 1 task
- Launch Type: Fargate on-demand (other agents use Spot)
- Task CPU: 2048 units (2 vCPU)
- Task Memory: 4096 MB (4 GB)
- Deployment Strategy: Rolling update (no circuit breaker configured)
- Health Check Grace Period: 60s
- Auto-scaling: Enabled (min: 1, max: 3 tasks, target: 75% CPU)

**Task Definition**: `karmacadabra-prod-facilitator` (revision history in ECR)
- Family: `karmacadabra-prod-facilitator`
- Network Mode: `awsvpc`
- Container Port: 8080
- Health Check: `curl -f http://localhost:8080/health || exit 1` (30s interval, 3 retries, 60s start period)

**Key Observation**: Facilitator is the ONLY service using on-demand Fargate (stability requirement). All other agents use Fargate Spot for cost savings.

---

### 2. Container Image & Registry

**ECR Repository**: `karmacadabra/facilitator`
- Full URI: `518898403364.dkr.ecr.us-east-1.amazonaws.com/karmacadabra/facilitator:latest`
- Image Scanning: Enabled (scan on push)
- Encryption: AES256 (free tier)
- Lifecycle Policy: Keep last 5 tagged images, delete untagged after 7 days
- Current Image: Built from `x402-rs/Dockerfile` with custom branding

**Build Process**: `scripts/build-and-push.py`
```python
# Facilitator-specific build config
'facilitator': {
    'context': 'x402-rs',
    'dockerfile': 'x402-rs/Dockerfile',
    'use_prebuilt': False,  # Build from source (Ultravioleta branding)
    'platform': 'linux/amd64'
}
```

**Critical**: Facilitator uses custom Dockerfile with:
- Nightly Rust toolchain (Edition 2024 compatibility)
- Custom landing page (`static/index.html` - 57KB Ultravioleta DAO branding)
- 17 funded networks (Base, Avalanche, Celo, HyperEVM, Polygon, Solana, Optimism + testnets)
- Handler modifications (`include_str!("../static/index.html")`)

**Extraction Requirement**: ECR repository can be standalone or shared. Recommend **standalone ECR** for clean separation.

---

### 3. AWS Secrets Manager Dependencies

**Secret 1**: `karmacadabra-facilitator-mainnet`
- ARN: `arn:aws:secretsmanager:us-east-1:518898403364:secret:karmacadabra-facilitator-mainnet-WTvZkf`
- Description: "Facilitator hot wallet for mainnet networks (Avalanche Mainnet, Base Mainnet)"
- Last Changed: 2025-10-26 15:33:41
- Structure (JSON):
  ```json
  {
    "private_key": "0x..."  // EVM private key for EIP-3009 transactions
  }
  ```
- Used for: Signing blockchain transactions on EVM networks (Base, Avalanche, Polygon, Celo, etc.)

**Secret 2**: `karmacadabra-solana-keypair`
- ARN: `arn:aws:secretsmanager:us-east-1:518898403364:secret:karmacadabra-solana-keypair-yWgz6P`
- Description: "Solana keypair for facilitator (mainnet)"
- Last Changed: 2025-10-29 19:01:55
- Structure (JSON):
  ```json
  {
    "private_key": "[base64_encoded_keypair]"  // Solana keypair bytes
  }
  ```
- Used for: Solana blockchain transactions

**Secret 3** (Optional): `karmacadabra-quicknode-base-rpc` (referenced in task-def-v2)
- Premium RPC endpoint for Base network (higher rate limits)
- Fallback: Public RPC `https://mainnet.base.org` (in environment variables)

**Extraction Strategy**:
- **Option A** (Recommended): Create new secret with same structure in target AWS account
  - Secret name: `x402-facilitator-mainnet` (or configurable via Terraform variable)
  - Migrate private keys manually (one-time secure transfer)
  - Update task definition to reference new secret ARN

- **Option B**: Keep shared secrets (if deploying in same AWS account)
  - Update IAM policies to grant new service access
  - Risk: Coupling between projects

**Security Note**: Never expose private keys in logs, task definitions, or Terraform state. Use `valueFrom` with Secrets Manager ARN only.

---

### 4. Network Configuration

**VPC**: `karmacadabra-prod` (SHARED)
- CIDR: `10.0.0.0/16`
- Availability Zones: `us-east-1a`, `us-east-1b`
- DNS Hostnames: Enabled
- DNS Support: Enabled
- Cost: Free (VPC itself)

**Subnets**:
- **Public Subnets** (2): `10.0.0.0/24`, `10.0.1.0/24`
  - Used for: ALB, NAT Gateway
  - Internet Gateway attached

- **Private Subnets** (2): `10.0.100.0/24`, `10.0.101.0/24`
  - Used for: ECS Fargate tasks (facilitator runs here)
  - Outbound via NAT Gateway
  - Current Facilitator Subnets: `subnet-0eb54a6ce2fee574a`, `subnet-0e53bcd040dfd80b5`

**NAT Gateway**: Single NAT in `us-east-1a` (COST OPTIMIZATION)
- Elastic IP: Allocated
- Cost: ~$32/month + data transfer (~$0.045/GB)
- Alternative: VPC Endpoints for AWS services (reduces NAT traffic)

**Security Groups**:

1. **ALB Security Group** (`sg-0...` - shared)
   - Inbound: HTTP (80), HTTPS (443) from `0.0.0.0/0`
   - Outbound: All traffic

2. **ECS Tasks Security Group** (`sg-0e147495bdc3e6f18` - shared)
   - Inbound: All TCP from ALB security group
   - Inbound: All TCP from self (inter-container communication)
   - Outbound: All traffic (for RPC calls, OpenAI API, etc.)
   - **Critical**: Facilitator needs outbound HTTPS to 17+ blockchain RPC endpoints

**VPC Endpoints** (Cost Optimization):
- `ecr.api` - Interface endpoint (pull Docker images)
- `ecr.dkr` - Interface endpoint (Docker registry)
- `s3` - Gateway endpoint (FREE)
- `logs` - Interface endpoint (CloudWatch Logs)
- `secretsmanager` - Interface endpoint (fetch secrets)
- Cost: ~$7/month per interface endpoint (5 endpoints = ~$35/month savings on NAT data transfer)

**Extraction Options**:

**Option A - Shared VPC** (Recommended for same AWS account):
- Facilitator continues using existing VPC
- Lower cost (~$0 additional networking costs)
- Complexity: VPC dependency in Terraform (data source)
- Use case: Deploying in same AWS account, agents need facilitator

**Option B - Standalone VPC**:
- Create new VPC for facilitator (independent infrastructure)
- Cost: +$32/month (NAT Gateway) + ~$35/month (VPC endpoints) = +$67/month
- Benefit: Complete isolation, portable across AWS accounts
- Use case: Deploying to different AWS account or region

**Option C - Hybrid** (Best for extraction):
- Terraform module accepts VPC ID as variable (optional)
- If VPC ID provided → use existing VPC (Option A)
- If VPC ID not provided → create new VPC (Option B)
- Default: Create new VPC (standalone deployment)

---

### 5. Load Balancer Configuration

**Application Load Balancer**: `karmacadabra-prod-alb` (SHARED)
- DNS: `karmacadabra-prod-alb-1234567890.us-east-1.elb.amazonaws.com`
- Scheme: Internet-facing
- IP Address Type: IPv4
- Subnets: Public subnets (`us-east-1a`, `us-east-1b`)
- Security Groups: ALB security group
- Idle Timeout: 180s (CRITICAL - increased from 60s for Base mainnet tx confirmations)
- Cost: ~$16.20/month (base) + $0.008/LCU-hour (~$6/month) = ~$22/month total

**Target Group**: `facili...` (name truncated, prefix-based naming)
- ARN: `arn:aws:elasticloadbalancing:us-east-1:518898403364:targetgroup/facili20251026204401441600000016/6443217ea4cd98e0`
- Protocol: HTTP
- Port: 8080
- Target Type: `ip` (required for Fargate)
- VPC: `karmacadabra-prod`
- Health Check:
  - Path: `/health`
  - Interval: 30s
  - Timeout: 5s
  - Healthy Threshold: 2
  - Unhealthy Threshold: 3
  - Matcher: 200
- Deregistration Delay: 30s
- Stickiness: Disabled

**Listener Rules**:

1. **HTTP Listener (Port 80)**:
   - Default Action: Redirect to HTTPS (301)
   - Facilitator Rule (Priority 10):
     - Condition: `Host: facilitator.ultravioletadao.xyz`
     - Action: Forward to facilitator target group

2. **HTTPS Listener (Port 443)**:
   - Certificate: ACM certificate for `*.karmacadabra.ultravioletadao.xyz` + `facilitator.ultravioletadao.xyz`
   - SSL Policy: `ELBSecurityPolicy-TLS-1-2-2017-01`
   - Default Action: Fixed response 404 (JSON error message)
   - Facilitator Rule (Priority 10):
     - Condition: `Host: facilitator.ultravioletadao.xyz`
     - Action: Forward to facilitator target group

**Path-based Routing** (Alternative):
- Not used for facilitator (uses hostname-based)
- Other agents use path-based: `/validator/*`, `/karma-hello/*`, etc.

**Extraction Challenge**: ALB is SHARED by all agents. Options:

**Option A - Shared ALB** (Zero-cost migration):
- Keep facilitator on existing ALB
- No DNS changes
- No downtime
- Complexity: Terraform manages subset of ALB rules
- Risk: Terraform state conflicts if both repos manage same ALB

**Option B - Standalone ALB**:
- Create new ALB for facilitator only
- Cost: +$22/month
- DNS change required: Update Route53 `facilitator.ultravioletadao.xyz` → new ALB
- Downtime: None (blue-green with Route53 weighted routing)
- Benefit: Complete isolation

**Option C - No ALB** (API Gateway or direct ECS Service Connect):
- Use API Gateway HTTP API (~$1/million requests)
- Or expose ECS service directly via Service Connect
- Cost: Variable (cheaper for low traffic)
- Complexity: Different architecture, may require code changes
- Use case: Greenfield deployment, not migration

**Recommendation**: Option A for migration, Option B for long-term standalone deployment.

---

### 6. DNS & TLS Configuration

**Route53 Hosted Zone**: `ultravioletadao.xyz` (SHARED)
- Managed outside this infrastructure (parent domain)
- Cost: $0.50/month per hosted zone + $0.40/million queries

**DNS Record**: `facilitator.ultravioletadao.xyz`
- Type: A (Alias)
- Target: ALB DNS name (`karmacadabra-prod-alb-...`)
- Evaluate Target Health: True
- TTL: 300s (CloudFormation default)
- Created by: `terraform/ecs-fargate/route53.tf`

**ACM Certificate**: Wildcard + SAN
- Domain: `karmacadabra.ultravioletadao.xyz`
- Subject Alternative Names:
  - `*.karmacadabra.ultravioletadao.xyz` (all agent subdomains)
  - `facilitator.ultravioletadao.xyz` (facilitator at root)
- Validation: DNS (Route53 automatic validation records)
- Auto-renewal: Enabled
- Cost: Free

**Extraction Strategy**:

**Option A - Keep Shared Domain**:
- `facilitator.ultravioletadao.xyz` continues pointing to same/new ALB
- No code changes in clients (seller agents, test scripts)
- Requires: Access to `ultravioletadao.xyz` hosted zone

**Option B - New Domain**:
- Example: `facilitator.x402.io` or `pay.ultravioletadao.xyz`
- Cost: $12/year (domain) + $0.50/month (hosted zone)
- Breaking change: All agents must update facilitator URL
- Benefit: Complete independence from parent project

**Option C - Dual DNS** (Migration path):
- Point both old and new domains to same ALB during transition
- Gradual client migration
- Deprecate old domain after 6-12 months

**TLS Certificate Extraction**:
- Create new ACM certificate for standalone domain
- Validation: DNS (requires Route53 access or manual validation)
- Cost: Free (AWS ACM)

---

### 7. IAM Roles & Policies

**Task Execution Role**: `karmacadabra-prod-ecs-exec-20251026204347677000000004`
- ARN: `arn:aws:iam::518898403364:role/karmacadabra-prod-ecs-exec-20251026204347677000000004`
- Purpose: Used by ECS agent to start containers (pull images, write logs, fetch secrets)
- Managed Policies:
  - `AmazonECSTaskExecutionRolePolicy` (AWS managed)
- Inline Policies:
  1. **Secrets Manager Access**:
     ```json
     {
       "Effect": "Allow",
       "Action": [
         "secretsmanager:GetSecretValue",
         "secretsmanager:DescribeSecret"
       ],
       "Resource": "arn:aws:secretsmanager:us-east-1:*:secret:karmacadabra-*"
     }
     ```
  2. **ECR Access**:
     ```json
     {
       "Effect": "Allow",
       "Action": [
         "ecr:GetAuthorizationToken",
         "ecr:BatchCheckLayerAvailability",
         "ecr:GetDownloadUrlForLayer",
         "ecr:BatchGetImage"
       ],
       "Resource": "*"
     }
     ```
  3. **KMS Decrypt** (for encrypted secrets):
     ```json
     {
       "Effect": "Allow",
       "Action": ["kms:Decrypt", "kms:DescribeKey"],
       "Resource": "*",
       "Condition": {
         "StringEquals": {
           "kms:ViaService": "secretsmanager.us-east-1.amazonaws.com"
         }
       }
     }
     ```

**Task Role**: `karmacadabra-prod-ecs-task-20251026204347632600000003`
- ARN: `arn:aws:iam::518898403364:role/karmacadabra-prod-ecs-task-20251026204347632600000003`
- Purpose: Used by running container application (facilitator Rust code)
- Inline Policies:
  1. **Secrets Manager Access** (runtime secret fetching - if needed):
     - Same as execution role
  2. **CloudWatch Logs**:
     ```json
     {
       "Effect": "Allow",
       "Action": [
         "logs:CreateLogGroup",
         "logs:CreateLogStream",
         "logs:PutLogEvents",
         "logs:DescribeLogStreams"
       ],
       "Resource": "arn:aws:logs:us-east-1:*:log-group:/ecs/karmacadabra-prod*"
     }
     ```
  3. **X-Ray Tracing** (if enabled):
     ```json
     {
       "Effect": "Allow",
       "Action": [
         "xray:PutTraceSegments",
         "xray:PutTelemetryRecords"
       ],
       "Resource": "*"
     }
     ```
  4. **ECS Exec** (debugging - SSH into containers):
     ```json
     {
       "Effect": "Allow",
       "Action": [
         "ssmmessages:CreateControlChannel",
         "ssmmessages:CreateDataChannel",
         "ssmmessages:OpenControlChannel",
         "ssmmessages:OpenDataChannel"
       ],
       "Resource": "*"
     }
     ```
  5. **S3 Access** (optional - for future data storage):
     ```json
     {
       "Effect": "Allow",
       "Action": ["s3:GetObject", "s3:PutObject", "s3:ListBucket"],
       "Resource": [
         "arn:aws:s3:::karmacadabra-prod-*",
         "arn:aws:s3:::karmacadabra-prod-*/*"
       ]
     }
     ```

**Extraction Requirements**:
- Create new IAM roles with same policies
- Update resource ARNs to match new infrastructure
- Principle of least privilege: Remove S3 access if not used by facilitator
- Consider: Facilitator does NOT use OpenAI API (unlike agents), so simpler permissions

**Critical Security Note**: Task execution role fetches secrets at container start. Task role is used by running application. Facilitator only needs:
- Execution Role: ECR pull, Secrets Manager fetch, CloudWatch Logs write
- Task Role: CloudWatch Logs write (runtime), ECS Exec (debugging)

---

### 8. CloudWatch Monitoring & Logging

**Log Group**: `/ecs/karmacadabra-prod/facilitator`
- Retention: 7 days (cost optimization)
- Log Stream Prefix: `ecs`
- Example Stream: `ecs/facilitator/1d425dfe-0980-4db0-86e1-6f25ee37b625`
- Size: ~50-100 MB/month (low traffic)
- Cost: ~$0.50/month (ingestion) + $0.03/GB storage

**Log Configuration** (in task definition):
```json
{
  "logDriver": "awslogs",
  "options": {
    "awslogs-group": "/ecs/karmacadabra-prod/facilitator",
    "awslogs-region": "us-east-1",
    "awslogs-stream-prefix": "ecs"
  }
}
```

**Metric Filters**:
- Error Count: Pattern `[time, request_id, level = ERROR*, ...]`
- Namespace: `karmacadabra/prod`
- Metric: `facilitatorErrorCount`

**CloudWatch Alarms**:

1. **High CPU Alarm**:
   - Metric: `ECSServiceAverageCPUUtilization`
   - Threshold: 85%
   - Evaluation Periods: 2 (10 minutes)
   - Action: SNS notification (if configured)

2. **High Memory Alarm**:
   - Metric: `ECSServiceAverageMemoryUtilization`
   - Threshold: 85%
   - Evaluation Periods: 2 (10 minutes)

3. **Low Task Count Alarm**:
   - Metric: `RunningTaskCount`
   - Threshold: < 1
   - Evaluation Periods: 1 (1 minute)
   - Action: Alert immediately if no tasks running

4. **Unhealthy Target Alarm** (ALB):
   - Metric: `UnHealthyHostCount`
   - Threshold: > 0
   - Evaluation Periods: 2 (2 minutes)
   - Dimensions: TargetGroup, LoadBalancer

**Container Insights**: Enabled (cluster-level)
- Cost: ~$3-5/month (custom metrics)
- Metrics: Task-level CPU, memory, network, disk I/O
- Benefit: Deep observability for troubleshooting

**X-Ray Tracing**: Configured but optional
- Sampling Rate: 5% of requests
- Cost: ~$5/100K traces

**Extraction Strategy**:
- Create new CloudWatch Log Group with same retention
- Recreate alarms with new service ARNs
- Consider: Unified logging across projects (ship logs to centralized S3/CloudWatch)
- Cost optimization: Use CloudWatch Logs Insights instead of Container Insights for standalone deployment

---

### 9. Auto-Scaling Configuration

**Auto-Scaling Target**:
- Service: `service/karmacadabra-prod/karmacadabra-prod-facilitator`
- Scalable Dimension: `ecs:service:DesiredCount`
- Min Capacity: 1 task
- Max Capacity: 3 tasks
- Current Desired: 1 task

**Scaling Policies**:

1. **CPU-based Scaling**:
   - Type: Target Tracking
   - Metric: `ECSServiceAverageCPUUtilization`
   - Target Value: 75%
   - Scale-out Cooldown: 60s
   - Scale-in Cooldown: 300s (5 minutes)
   - Behavior: Add task if CPU > 75% for 2 consecutive periods (10 min)

2. **Memory-based Scaling**:
   - Type: Target Tracking
   - Metric: `ECSServiceAverageMemoryUtilization`
   - Target Value: 80%
   - Scale-out Cooldown: 60s
   - Scale-in Cooldown: 300s

**Observed Behavior**:
- Facilitator rarely scales beyond 1 task (low traffic ~50 RPS)
- CPU typically 10-20%, Memory 30-40%
- Auto-scaling provides buffer for traffic spikes (live stream demos, testnet stress tests)

**Extraction Recommendation**:
- Keep same auto-scaling configuration (1-3 tasks)
- Consider: Scheduled scaling if traffic patterns are predictable
- Cost impact: Minimal (only pay for tasks when scaled)

---

### 10. Deployment Pipeline & Workflows

**Current Deployment Process**:

1. **Build Docker Image**: `scripts/build-and-push.py facilitator`
   - Builds from `x402-rs/Dockerfile`
   - Tags as `518898403364.dkr.ecr.us-east-1.amazonaws.com/karmacadabra/facilitator:latest`
   - Pushes to ECR
   - Duration: ~3-5 minutes

2. **Deploy to ECS**: `scripts/deploy-to-fargate.py --force-deploy facilitator`
   - Runs `terraform apply` (updates infrastructure)
   - Forces ECS service redeployment (pulls latest `:latest` tag)
   - Waits for service stability (optional with `--wait`)
   - Duration: ~5-10 minutes

3. **Verify Deployment**: `scripts/test_all_endpoints.py`
   - Tests `/health`, `/supported`, `/networks` endpoints
   - Validates facilitator responds correctly
   - Duration: ~10 seconds

**Deployment Strategy**: Rolling Update
- ECS launches new task with new image
- Waits for health check to pass (60s start period + 3x30s checks)
- Drains connections from old task (30s deregistration delay)
- Terminates old task
- Total deployment time: ~2-3 minutes
- Downtime: None (overlapping tasks)

**CI/CD Integration**: Manual (no GitHub Actions/CodePipeline currently)
- Future enhancement: GitHub Actions on push to `master`
- Trigger: `docker build` → `docker push` → `aws ecs update-service`

**Terraform State**:
- Backend: S3 bucket `karmacadabra-terraform-state`
- Key: `ecs-fargate/terraform.tfstate`
- Lock: DynamoDB table `karmacadabra-terraform-locks`
- Encryption: Enabled (SSE-S3)

**Extraction Challenge**: Terraform state is SHARED. Options:

**Option A - Separate Terraform State**:
- Create new S3 bucket for facilitator: `x402-facilitator-terraform-state`
- New DynamoDB lock table: `x402-facilitator-terraform-locks`
- Benefit: Complete isolation, no state conflicts
- Cost: +$0.50/month (minimal)

**Option B - Shared State with Workspaces**:
- Use Terraform workspaces: `terraform workspace new facilitator-standalone`
- Same backend, isolated state files
- Complexity: Workspace management
- Risk: Accidental cross-workspace changes

**Option C - Terraform State Migration**:
- Extract facilitator resources from shared state
- Import into new state: `terraform import`
- Requires: Downtime or careful state surgery
- Risk: High (can break production if done incorrectly)

**Recommendation**: Option A (separate state) for clean extraction.

---

### 11. Cost Analysis

**Current Monthly Cost (Facilitator-specific)**:

| Resource | Cost | Notes |
|----------|------|-------|
| **Fargate Task (2 vCPU, 4 GB)** | $39.73 | On-demand, 730 hours/month, 1 task |
| **ECR Storage** | $0.50 | ~5 GB images (5 versions × 1 GB) |
| **CloudWatch Logs** | $0.53 | ~50 MB/month ingestion + 7-day retention |
| **CloudWatch Alarms** | $0.40 | 4 alarms × $0.10/alarm |
| **NAT Gateway (Shared)** | $5.33 | ~$32/month ÷ 6 agents (facilitator share) |
| **ALB (Shared)** | $3.67 | ~$22/month ÷ 6 agents |
| **VPC Endpoints (Shared)** | $5.83 | ~$35/month ÷ 6 agents (5 interface endpoints) |
| **Secrets Manager** | $0.80 | 2 secrets × $0.40/month |
| **Data Transfer** | $2.00 | ~100 GB/month × $0.02/GB (RPC calls, agent responses) |
| **Container Insights** | $0.50 | ~$3/month ÷ 6 agents |
| **TOTAL (Shared Infrastructure)** | **$59.29/month** | Facilitator share of shared resources |

**Standalone Deployment Cost** (if deployed independently):

| Resource | Cost | Notes |
|----------|------|-------|
| **Fargate Task** | $39.73 | Same as current |
| **ECR Storage** | $0.50 | Same |
| **CloudWatch Logs** | $0.53 | Same |
| **CloudWatch Alarms** | $0.40 | Same |
| **NAT Gateway** | $32.00 | Dedicated NAT (not shared) |
| **ALB** | $22.00 | Dedicated ALB (not shared) |
| **VPC Endpoints** | $35.00 | 5 interface endpoints (ECR, Logs, Secrets, S3 gateway free) |
| **Secrets Manager** | $0.80 | Same |
| **Data Transfer** | $2.00 | Same |
| **TOTAL (Standalone)** | **$132.96/month** | Full infrastructure cost |

**Cost Delta**: +$73.67/month (+124%) for standalone deployment

**Cost Optimization Strategies**:

1. **Eliminate NAT Gateway** (saves $32/month):
   - Use VPC endpoints for all AWS services
   - Limitation: Cannot call external RPC endpoints (blockchain networks)
   - Workaround: Use AWS PrivateLink for RPC providers (if supported)
   - **Not Recommended** for facilitator (needs 17+ RPC endpoints)

2. **Use Fargate Spot** (saves ~$28/month, 70% savings):
   - Risk: Task interruptions (2-minute notice)
   - Mitigation: Run 2 tasks (1 on-demand, 1 Spot) for redundancy
   - **Not Recommended** for facilitator (payment processor, high availability requirement)

3. **Reduce Fargate Task Size** (saves ~$20/month):
   - Downgrade to 1 vCPU / 2 GB (from 2 vCPU / 4 GB)
   - Risk: Performance degradation under load
   - Test: Monitor CPU/memory usage, stress test
   - **Potentially Feasible** (current usage: 10-20% CPU, 30-40% memory)

4. **Share ALB with Other Services** (saves $22/month):
   - If deploying facilitator + other microservices in same account
   - Use path-based or host-based routing
   - **Recommended** if multiple services

5. **Use Smaller Log Retention** (saves ~$0.25/month):
   - Change from 7 days to 3 days
   - Minimal savings, keeps troubleshooting data

**Recommended Configuration for Standalone Deployment**:
- **Phase 1** (Migration): Keep shared infrastructure ($59/month)
- **Phase 2** (Standalone): Dedicated ALB + NAT, optimize task size ($110/month)
- **Phase 3** (Long-term): Task size optimization + monitoring ($90-100/month)

---

## Extraction Strategy & Migration Plan

### Approach: Terraform Module with Multi-Deployment Support

**Architecture Decision**: Create a **reusable Terraform module** that supports:
1. **Standalone Deployment**: Provisions all infrastructure (VPC, ALB, ECS, etc.)
2. **Shared Deployment**: Uses existing infrastructure (data sources for VPC, ALB)
3. **Hybrid Deployment**: Mix of new and existing resources (e.g., new ECS service in existing VPC)

**Module Structure**:
```
x402-facilitator/
├── terraform/
│   ├── modules/
│   │   ├── networking/       # VPC, subnets, NAT, security groups
│   │   ├── load-balancer/    # ALB, target groups, listeners
│   │   ├── ecs-service/      # ECS cluster, task definition, service
│   │   ├── observability/    # CloudWatch logs, alarms, X-Ray
│   │   └── dns/              # Route53, ACM certificates
│   ├── environments/
│   │   ├── standalone/       # Full stack deployment
│   │   └── shared/           # Deploy into existing infrastructure
│   └── main.tf               # Root module (orchestrates submodules)
├── scripts/
│   ├── build-and-push.sh     # Docker build + ECR push
│   ├── deploy.sh             # Terraform apply + ECS update
│   └── test-endpoints.sh     # Health check validation
└── x402-rs/                  # Facilitator application code
```

---

### Migration Steps (Zero-Downtime)

**Phase 1: Prepare Extraction (Week 1)**

1. **Create New Repository**:
   ```bash
   gh repo create ultravioleta/x402-facilitator --private
   cd x402-facilitator
   ```

2. **Copy Facilitator Code**:
   ```bash
   # Copy x402-rs directory
   cp -r /path/to/karmacadabra/x402-rs ./x402-rs

   # Copy critical configuration
   cp /path/to/karmacadabra/x402-rs/Dockerfile ./x402-rs/
   cp -r /path/to/karmacadabra/x402-rs/static ./x402-rs/
   ```

3. **Extract Terraform Configuration**:
   ```bash
   # Create module structure
   mkdir -p terraform/modules/{networking,load-balancer,ecs-service,observability,dns}

   # Extract facilitator-specific resources from karmacadabra/terraform/ecs-fargate/
   # See detailed extraction checklist below
   ```

4. **Create New AWS Secrets** (in target AWS account):
   ```bash
   # Create facilitator mainnet secret
   aws secretsmanager create-secret \
     --name x402-facilitator-mainnet \
     --description "Facilitator hot wallet for mainnet networks" \
     --secret-string '{"private_key":"MIGRATE_FROM_OLD_SECRET"}' \
     --region us-east-1

   # Create Solana keypair secret
   aws secretsmanager create-secret \
     --name x402-solana-keypair \
     --description "Solana keypair for facilitator" \
     --secret-string '{"private_key":"MIGRATE_FROM_OLD_SECRET"}' \
     --region us-east-1

   # Migrate private keys securely (one-time manual operation)
   # DO NOT log keys, use AWS CLI with --query to extract
   ```

5. **Create Terraform Backend**:
   ```bash
   # S3 bucket for state
   aws s3api create-bucket \
     --bucket x402-facilitator-terraform-state \
     --region us-east-1

   # Enable versioning
   aws s3api put-bucket-versioning \
     --bucket x402-facilitator-terraform-state \
     --versioning-configuration Status=Enabled

   # Enable encryption
   aws s3api put-bucket-encryption \
     --bucket x402-facilitator-terraform-state \
     --server-side-encryption-configuration \
       '{"Rules":[{"ApplyServerSideEncryptionByDefault":{"SSEAlgorithm":"AES256"}}]}'

   # DynamoDB table for locking
   aws dynamodb create-table \
     --table-name x402-facilitator-terraform-locks \
     --attribute-definitions AttributeName=LockID,AttributeType=S \
     --key-schema AttributeName=LockID,KeyType=HASH \
     --billing-mode PAY_PER_REQUEST \
     --region us-east-1
   ```

**Phase 2: Parallel Deployment (Week 2)**

6. **Deploy Standalone Infrastructure** (in separate AWS account or region):
   ```bash
   cd terraform/environments/standalone

   # Initialize Terraform
   terraform init \
     -backend-config="bucket=x402-facilitator-terraform-state" \
     -backend-config="key=standalone/terraform.tfstate" \
     -backend-config="region=us-east-1"

   # Review plan
   terraform plan -out=tfplan

   # Apply (creates VPC, ALB, ECS cluster, etc.)
   terraform apply tfplan
   ```

7. **Build and Push Docker Image**:
   ```bash
   # Build facilitator image
   cd x402-rs
   docker build -t x402-facilitator:latest .

   # Tag for new ECR
   docker tag x402-facilitator:latest \
     518898403364.dkr.ecr.us-east-1.amazonaws.com/x402-facilitator/facilitator:latest

   # Push to ECR
   aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin 518898403364.dkr.ecr.us-east-1.amazonaws.com
   docker push 518898403364.dkr.ecr.us-east-1.amazonaws.com/x402-facilitator/facilitator:latest
   ```

8. **Test Standalone Deployment**:
   ```bash
   # Wait for service to stabilize
   aws ecs wait services-stable \
     --cluster x402-facilitator-prod \
     --services x402-facilitator-prod-facilitator \
     --region us-east-1

   # Test health endpoint
   curl https://facilitator-new.ultravioletadao.xyz/health

   # Test payment flow
   python scripts/test_all_endpoints.py --url https://facilitator-new.ultravioletadao.xyz
   ```

**Phase 3: DNS Migration (Week 3)**

9. **Blue-Green Deployment with Route53**:
   ```bash
   # Create new DNS record with weighted routing
   aws route53 change-resource-record-sets \
     --hosted-zone-id Z1234567890ABC \
     --change-batch '{
       "Changes": [{
         "Action": "CREATE",
         "ResourceRecordSet": {
           "Name": "facilitator.ultravioletadao.xyz",
           "Type": "A",
           "SetIdentifier": "new-facilitator",
           "Weight": 10,
           "AliasTarget": {
             "HostedZoneId": "Z35SXDOTRQ7X7K",
             "DNSName": "x402-facilitator-alb-new.us-east-1.elb.amazonaws.com",
             "EvaluateTargetHealth": true
           }
         }
       }]
     }'

   # Monitor traffic to new deployment (10% weight)
   # Check CloudWatch metrics, error rates, latency

   # If successful, gradually increase weight (20% → 50% → 100%)
   # If issues, set weight to 0 (instant rollback)
   ```

10. **Monitor and Validate**:
    ```bash
    # Monitor CloudWatch Logs
    aws logs tail /ecs/x402-facilitator-prod/facilitator --follow

    # Monitor ECS service health
    aws ecs describe-services \
      --cluster x402-facilitator-prod \
      --services x402-facilitator-prod-facilitator \
      --query 'services[0].[runningCount,desiredCount,deployments]'

    # Monitor ALB target health
    aws elbv2 describe-target-health \
      --target-group-arn arn:aws:elasticloadbalancing:...

    # Run full payment flow test
    cd /path/to/karmacadabra
    python scripts/test_glue_payment_simple.py \
      --facilitator https://facilitator.ultravioletadao.xyz \
      --network avalanche-fuji
    ```

**Phase 4: Decommission Old Deployment (Week 4)**

11. **Remove Facilitator from Shared Infrastructure**:
    ```bash
    # In karmacadabra repository
    cd terraform/ecs-fargate

    # Remove facilitator from agents map in variables.tf
    # Comment out facilitator in var.agents

    # Plan removal (REVIEW CAREFULLY)
    terraform plan -out=tfplan

    # Apply (destroys facilitator ECS service, target group, Route53 record)
    terraform apply tfplan
    ```

12. **Archive Old Secrets** (optional):
    ```bash
    # Mark old secrets for deletion (30-day recovery period)
    aws secretsmanager delete-secret \
      --secret-id karmacadabra-facilitator-mainnet \
      --recovery-window-in-days 30 \
      --region us-east-1

    # Or keep for backup (no cost for unused secrets)
    ```

---

### Terraform Extraction Checklist

**Resources to Extract**:

From `terraform/ecs-fargate/main.tf`:
- [ ] `aws_ecs_task_definition.agents["facilitator"]` → `modules/ecs-service/task-definition.tf`
- [ ] `aws_ecs_service.agents["facilitator"]` → `modules/ecs-service/service.tf`
- [ ] `aws_appautoscaling_target.ecs_service["facilitator"]` → `modules/ecs-service/autoscaling.tf`
- [ ] `aws_appautoscaling_policy.cpu["facilitator"]` → `modules/ecs-service/autoscaling.tf`
- [ ] `aws_appautoscaling_policy.memory["facilitator"]` → `modules/ecs-service/autoscaling.tf`

From `terraform/ecs-fargate/ecr.tf`:
- [ ] `aws_ecr_repository.agents["facilitator"]` → `modules/ecs-service/ecr.tf`
- [ ] `aws_ecr_lifecycle_policy.agents["facilitator"]` → `modules/ecs-service/ecr.tf`

From `terraform/ecs-fargate/alb.tf`:
- [ ] `aws_lb_target_group.agents["facilitator"]` → `modules/load-balancer/target-group.tf`
- [ ] `aws_lb_listener_rule.facilitator_root_http` → `modules/load-balancer/listener-rules.tf`
- [ ] `aws_lb_listener_rule.facilitator_root_https` → `modules/load-balancer/listener-rules.tf`

From `terraform/ecs-fargate/cloudwatch.tf`:
- [ ] `aws_cloudwatch_log_group.agents["facilitator"]` → `modules/observability/logs.tf`
- [ ] `aws_cloudwatch_log_metric_filter.error_count["facilitator"]` → `modules/observability/metrics.tf`
- [ ] `aws_cloudwatch_metric_alarm.high_cpu["facilitator"]` → `modules/observability/alarms.tf`
- [ ] `aws_cloudwatch_metric_alarm.high_memory["facilitator"]` → `modules/observability/alarms.tf`
- [ ] `aws_cloudwatch_metric_alarm.low_task_count["facilitator"]` → `modules/observability/alarms.tf`
- [ ] `aws_cloudwatch_metric_alarm.unhealthy_targets["facilitator"]` → `modules/observability/alarms.tf`

From `terraform/ecs-fargate/route53.tf`:
- [ ] `aws_route53_record.facilitator` → `modules/dns/route53.tf`

From `terraform/ecs-fargate/acm.tf`:
- [ ] Update `aws_acm_certificate.main` to include only facilitator domain → `modules/dns/acm.tf`

From `terraform/ecs-fargate/iam.tf`:
- [ ] `aws_iam_role.ecs_task_execution` (copy, adjust policies) → `modules/ecs-service/iam.tf`
- [ ] `aws_iam_role.ecs_task` (copy, adjust policies) → `modules/ecs-service/iam.tf`
- [ ] All inline policies (adjust resource ARNs) → `modules/ecs-service/iam.tf`

**Conditional Resources** (create if standalone, use data sources if shared):

From `terraform/ecs-fargate/vpc.tf`:
- [ ] `aws_vpc.main` → `modules/networking/vpc.tf` (or `data.aws_vpc` if shared)
- [ ] `aws_subnet.public` → `modules/networking/subnets.tf`
- [ ] `aws_subnet.private` → `modules/networking/subnets.tf`
- [ ] `aws_internet_gateway.main` → `modules/networking/igw.tf`
- [ ] `aws_nat_gateway.main` → `modules/networking/nat.tf`
- [ ] `aws_route_table.public` → `modules/networking/routes.tf`
- [ ] `aws_route_table.private` → `modules/networking/routes.tf`
- [ ] `aws_vpc_endpoint.*` → `modules/networking/vpc-endpoints.tf`

From `terraform/ecs-fargate/security_groups.tf`:
- [ ] `aws_security_group.alb` → `modules/load-balancer/security-groups.tf`
- [ ] `aws_security_group.ecs_tasks` → `modules/ecs-service/security-groups.tf`
- [ ] `aws_security_group.vpc_endpoints` → `modules/networking/security-groups.tf`

From `terraform/ecs-fargate/alb.tf`:
- [ ] `aws_lb.main` → `modules/load-balancer/alb.tf` (or `data.aws_lb` if shared)
- [ ] `aws_lb_listener.http` → `modules/load-balancer/listeners.tf`
- [ ] `aws_lb_listener.https` → `modules/load-balancer/listeners.tf`

**Data Sources** (for shared deployment):
```hcl
# modules/ecs-service/data.tf (shared deployment mode)
data "aws_vpc" "existing" {
  count = var.create_vpc ? 0 : 1
  id    = var.vpc_id
}

data "aws_lb" "existing" {
  count = var.create_alb ? 0 : 1
  arn   = var.alb_arn
}

data "aws_ecs_cluster" "existing" {
  count        = var.create_cluster ? 0 : 1
  cluster_name = var.cluster_name
}

data "aws_route53_zone" "main" {
  name         = var.hosted_zone_name
  private_zone = false
}
```

---

### Testing & Validation Scripts

**Extract from `karmacadabra/scripts/`**:

1. **Build Script**: `scripts/build-and-push.sh`
   - Adapt from `build-and-push.py`
   - Hardcode facilitator-only configuration
   - Remove multi-agent logic

2. **Deployment Script**: `scripts/deploy.sh`
   - Adapt from `deploy-to-fargate.py`
   - Terraform apply + ECS service update
   - Support `--standalone` and `--shared` flags

3. **Health Check**: `scripts/test-endpoints.sh`
   - Extract facilitator tests from `test_all_endpoints.py`
   - Test `/health`, `/supported`, `/networks`
   - Validate payment flow with `test_glue_payment_simple.py`

4. **Secrets Migration**: `scripts/migrate-secrets.sh`
   - Securely copy private keys from old to new secrets
   - Use AWS CLI with `--query` to avoid logging keys
   - Validate copied secrets

5. **Rollback Script**: `scripts/rollback.sh`
   - Revert DNS to old ALB (Route53 weighted routing)
   - Scale down new ECS service
   - Emergency recovery procedure

**New Scripts to Create**:

1. **`scripts/stress-test.sh`**:
   - Load test facilitator with 1000+ concurrent requests
   - Validate auto-scaling triggers correctly
   - Monitor CPU/memory usage

2. **`scripts/cost-estimate.sh`**:
   - Query AWS Cost Explorer API
   - Generate monthly cost report
   - Compare standalone vs shared deployment costs

3. **`scripts/validate-infrastructure.sh`**:
   - Run `terraform validate`
   - Run `tflint` (Terraform linter)
   - Check security group rules (no `0.0.0.0/0` ingress except ALB)
   - Validate IAM policies (least privilege)

---

## Risk Assessment & Mitigation

### High-Risk Items

**Risk 1: Downtime During Migration**
- **Impact**: Payment processing unavailable, agents cannot purchase services
- **Probability**: Medium (if not using blue-green deployment)
- **Mitigation**:
  - Use Route53 weighted routing (gradual cutover)
  - Deploy standalone infrastructure in parallel
  - Test thoroughly before DNS migration
  - Keep old infrastructure running for 1-2 weeks (rollback capability)

**Risk 2: Secret Key Exposure**
- **Impact**: Hot wallet compromise, funds stolen, DAO reputation damage
- **Probability**: Low (with proper procedures)
- **Mitigation**:
  - Never log private keys
  - Use AWS CLI `--query` to extract secrets programmatically
  - Rotate keys after migration (update all agent configs)
  - Enable CloudTrail for Secrets Manager access auditing
  - Use MFA for AWS console access during migration

**Risk 3: Terraform State Corruption**
- **Impact**: Infrastructure unmanageable, manual AWS console fixes required
- **Probability**: Medium (if extracting from shared state)
- **Mitigation**:
  - Backup Terraform state before extraction: `aws s3 cp s3://bucket/key local-backup.tfstate`
  - Use separate state backend (new S3 bucket)
  - Do NOT use `terraform state rm` on production (causes drift)
  - Test state extraction in non-production environment first

**Risk 4: Increased Costs**
- **Impact**: $73/month additional cost for standalone deployment
- **Probability**: High (confirmed by cost analysis)
- **Mitigation**:
  - Phase 1: Keep shared infrastructure (no cost increase)
  - Phase 2: Optimize task size (1 vCPU / 2 GB test)
  - Phase 3: Evaluate traffic patterns, consider Fargate Spot for dev/staging
  - Long-term: Share ALB with other microservices if available

### Medium-Risk Items

**Risk 5: DNS Propagation Delays**
- **Impact**: Some clients resolve old IP, others new IP (split brain)
- **Probability**: Medium (DNS caching, TTL issues)
- **Mitigation**:
  - Lower TTL to 60s before migration (1 week in advance)
  - Use Route53 alias records (instant failover)
  - Monitor both old and new endpoints during cutover
  - Communicate migration window to agent developers

**Risk 6: Dependency Coupling**
- **Impact**: Agents break if facilitator domain/endpoint changes
- **Probability**: Medium (hard-coded URLs in agent configs)
- **Mitigation**:
  - Keep facilitator domain unchanged: `facilitator.ultravioletadao.xyz`
  - If domain must change, support both old and new for 6 months
  - Document migration in agent README files
  - Provide backwards-compatible redirects (ALB rules)

**Risk 7: Monitoring Blind Spots**
- **Impact**: Production issues undetected, no alerting
- **Probability**: Medium (new CloudWatch alarms not configured correctly)
- **Mitigation**:
  - Test alarms before migration (manually trigger CPU spike)
  - Configure SNS topic for alerts (email/Slack/PagerDuty)
  - Enable Container Insights for deep visibility
  - Set up synthetic monitoring (CloudWatch Synthetics canaries)

### Low-Risk Items

**Risk 8: IAM Permission Gaps**
- **Impact**: ECS tasks fail to start (can't pull image, fetch secrets)
- **Probability**: Low (copying existing IAM policies)
- **Mitigation**:
  - Test IAM policies in dev environment first
  - Use AWS IAM Access Analyzer to identify unused permissions
  - Enable CloudTrail to debug permission errors
  - Start with broad permissions, tighten after validation

**Risk 9: Auto-Scaling Misconfiguration**
- **Impact**: Service under/over-scales, performance issues or cost spikes
- **Probability**: Low (copying existing configuration)
- **Mitigation**:
  - Set conservative max capacity (3 tasks)
  - Monitor scaling events in CloudWatch
  - Load test before production cutover
  - Disable auto-scaling initially, enable after observing traffic

**Risk 10: Certificate Expiry**
- **Impact**: HTTPS connections fail, facilitator unreachable
- **Probability**: Very Low (ACM auto-renews)
- **Mitigation**:
  - Use ACM (auto-renewal every 60 days)
  - Enable CloudWatch alarm for certificate expiry (< 30 days)
  - Validate DNS records for certificate validation
  - Test certificate after creation: `curl -vI https://facilitator...`

---

## Recommendations & Next Steps

### Immediate Actions (Week 1)

1. **Create New Repository**:
   ```bash
   gh repo create ultravioleta/x402-facilitator --private --description "Standalone x402 payment facilitator"
   ```

2. **Extract Terraform Modules**:
   - Copy `terraform/ecs-fargate/` to new repo
   - Refactor into modules (networking, load-balancer, ecs-service, observability, dns)
   - Add `create_vpc`, `create_alb` variables for shared deployment support

3. **Create AWS Secrets**:
   - Provision `x402-facilitator-mainnet` secret
   - Provision `x402-solana-keypair` secret
   - DO NOT migrate keys yet (wait for Phase 2)

4. **Set Up Terraform Backend**:
   - Create S3 bucket: `x402-facilitator-terraform-state`
   - Create DynamoDB table: `x402-facilitator-terraform-locks`
   - Enable versioning + encryption on S3

### Short-Term (Weeks 2-3)

5. **Deploy to Staging Environment**:
   - Use separate AWS account or region
   - Deploy full standalone stack
   - Test payment flows, stress test auto-scaling
   - Validate monitoring and alarms

6. **Migrate Secrets Securely**:
   - Script secure key migration
   - Use AWS CLI with IAM temporary credentials
   - Audit CloudTrail logs
   - Rotate keys after migration (best practice)

7. **Prepare DNS Migration**:
   - Lower TTL on `facilitator.ultravioletadao.xyz` to 60s
   - Create Route53 weighted routing configuration
   - Test with 1% traffic to new deployment

### Medium-Term (Month 2)

8. **Production Migration**:
   - Blue-green deployment: 10% → 50% → 100% traffic
   - Monitor error rates, latency, CloudWatch alarms
   - Rollback plan: Set weight to 0 instantly

9. **Decommission Old Deployment**:
   - Remove facilitator from `karmacadabra/terraform/ecs-fargate/`
   - Archive old secrets (30-day recovery window)
   - Update documentation to point to new repository

10. **Cost Optimization**:
    - Analyze CloudWatch Container Insights metrics
    - Test 1 vCPU / 2 GB task size (potential $20/month savings)
    - Evaluate Fargate Spot for dev/staging environments

### Long-Term (Month 3+)

11. **CI/CD Pipeline**:
    - GitHub Actions: Build Docker image on push to `main`
    - Automated testing: Unit tests, integration tests, e2e payment flow
    - Automated deployment: Terraform apply, ECS service update
    - Rollback automation: Deploy previous image on failure

12. **Multi-Region Deployment** (Future Enhancement):
    - Deploy to `us-west-2` for high availability
    - Route53 latency-based routing
    - Cross-region ECR replication
    - Estimated cost: +$130/month (full infrastructure in second region)

13. **Observability Enhancements**:
    - AWS X-Ray distributed tracing (already configured)
    - CloudWatch Synthetics canaries (synthetic monitoring)
    - CloudWatch Logs Insights dashboards
    - Integration with Datadog/New Relic (if needed)

---

## Decision Matrix

| Aspect | Option A: Shared Infrastructure | Option B: Standalone | Option C: Hybrid | Recommendation |
|--------|----------------------------------|----------------------|------------------|----------------|
| **VPC** | Use existing `karmacadabra-prod` VPC | Create new VPC | Configurable via variable | **Option C** (default: new VPC) |
| **ALB** | Keep on shared ALB | Create dedicated ALB | Configurable | **Option B** (clean separation) |
| **ECS Cluster** | Use `karmacadabra-prod` cluster | Create `x402-facilitator-prod` | Configurable | **Option B** (independent scaling) |
| **Secrets Manager** | Share secrets (same account) | New secrets (migration required) | N/A | **Option B** (security isolation) |
| **Route53** | Keep `facilitator.ultravioletadao.xyz` | New domain `facilitator.x402.io` | Dual DNS during migration | **Option A** (no client changes) |
| **ECR Repository** | Share `karmacadabra/facilitator` | New `x402-facilitator/facilitator` | N/A | **Option B** (clean separation) |
| **Terraform State** | Shared state (risk of conflicts) | Separate S3 backend | N/A | **Option B** (mandatory for safety) |
| **Cost** | $59/month (current) | $133/month (standalone) | Variable | **Optimize** (target: $90-110/month) |

**Overall Recommendation**: **Hybrid Approach**
- Terraform modules support both shared and standalone deployment
- Default configuration: Standalone (Option B) for clean separation
- Migration path: Shared (Option A) → Standalone (Option B) over 4 weeks
- Cost optimization: Task size reduction + infrastructure sharing where possible

---

## Appendix: Example Terraform Module

### `modules/ecs-service/variables.tf`

```hcl
variable "service_name" {
  description = "Name of the ECS service"
  type        = string
  default     = "x402-facilitator"
}

variable "environment" {
  description = "Environment name (prod, staging, dev)"
  type        = string
  default     = "prod"
}

variable "task_cpu" {
  description = "Fargate task CPU units (256, 512, 1024, 2048, 4096)"
  type        = number
  default     = 2048
}

variable "task_memory" {
  description = "Fargate task memory in MB"
  type        = number
  default     = 4096
}

variable "desired_count" {
  description = "Number of tasks to run"
  type        = number
  default     = 1
}

variable "create_cluster" {
  description = "Create new ECS cluster (false = use existing)"
  type        = bool
  default     = true
}

variable "cluster_name" {
  description = "ECS cluster name (if create_cluster = false)"
  type        = string
  default     = ""
}

variable "vpc_id" {
  description = "VPC ID (required if create_vpc = false in networking module)"
  type        = string
  default     = ""
}

variable "private_subnet_ids" {
  description = "Private subnet IDs for ECS tasks"
  type        = list(string)
  default     = []
}

variable "target_group_arn" {
  description = "ALB target group ARN"
  type        = string
}

variable "security_group_ids" {
  description = "Security group IDs for ECS tasks"
  type        = list(string)
  default     = []
}

variable "secrets_arns" {
  description = "Secrets Manager ARNs for private keys"
  type = object({
    evm_private_key    = string
    solana_private_key = string
  })
}

variable "ecr_repository_url" {
  description = "ECR repository URL for facilitator image"
  type        = string
}

variable "environment_variables" {
  description = "Environment variables for facilitator container"
  type = map(string)
  default = {
    PORT                     = "8080"
    HOST                     = "0.0.0.0"
    RUST_LOG                 = "info"
    SIGNER_TYPE              = "private-key"
    RPC_URL_BASE             = "https://mainnet.base.org"
    RPC_URL_AVALANCHE        = "https://avalanche-c-chain-rpc.publicnode.com"
    # ... (17 networks total)
  }
}
```

### `modules/ecs-service/main.tf`

```hcl
# ECS Cluster (conditional creation)
resource "aws_ecs_cluster" "main" {
  count = var.create_cluster ? 1 : 0

  name = "${var.service_name}-${var.environment}"

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = {
    Name        = "${var.service_name}-${var.environment}-cluster"
    Environment = var.environment
  }
}

# Use existing cluster (data source)
data "aws_ecs_cluster" "existing" {
  count = var.create_cluster ? 0 : 1

  cluster_name = var.cluster_name
}

locals {
  cluster_id   = var.create_cluster ? aws_ecs_cluster.main[0].id : data.aws_ecs_cluster.existing[0].arn
  cluster_name = var.create_cluster ? aws_ecs_cluster.main[0].name : var.cluster_name
}

# Task Definition
resource "aws_ecs_task_definition" "facilitator" {
  family                   = "${var.service_name}-${var.environment}"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "facilitator"
    image     = "${var.ecr_repository_url}:latest"
    essential = true

    portMappings = [{
      containerPort = 8080
      hostPort      = 8080
      protocol      = "tcp"
      name          = "facilitator"
    }]

    environment = [
      for key, value in var.environment_variables : {
        name  = key
        value = value
      }
    ]

    secrets = [
      {
        name      = "EVM_PRIVATE_KEY"
        valueFrom = "${var.secrets_arns.evm_private_key}:private_key::"
      },
      {
        name      = "SOLANA_PRIVATE_KEY"
        valueFrom = "${var.secrets_arns.solana_private_key}:private_key::"
      }
    ]

    healthCheck = {
      command     = ["CMD-SHELL", "curl -f http://localhost:8080/health || exit 1"]
      interval    = 30
      timeout     = 5
      retries     = 3
      startPeriod = 60
    }

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.facilitator.name
        "awslogs-region"        = data.aws_region.current.name
        "awslogs-stream-prefix" = "ecs"
      }
    }
  }])

  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }

  tags = {
    Name        = "${var.service_name}-${var.environment}-task"
    Environment = var.environment
  }
}

# ECS Service
resource "aws_ecs_service" "facilitator" {
  name            = "${var.service_name}-${var.environment}"
  cluster         = local.cluster_id
  task_definition = aws_ecs_task_definition.facilitator.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = var.private_subnet_ids
    security_groups  = var.security_group_ids
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = var.target_group_arn
    container_name   = "facilitator"
    container_port   = 8080
  }

  health_check_grace_period_seconds = 60
  force_new_deployment              = true
  propagate_tags                    = "TASK_DEFINITION"

  tags = {
    Name        = "${var.service_name}-${var.environment}-service"
    Environment = var.environment
  }

  depends_on = [
    aws_iam_role_policy.ecs_secrets_access,
    aws_iam_role_policy.task_cloudwatch_logs
  ]
}

data "aws_region" "current" {}
```

---

## Conclusion

The x402-rs facilitator extraction requires careful planning due to its tight integration with shared Karmacadabra infrastructure. The recommended **Terraform module approach** provides:

1. **Flexibility**: Deploy standalone or shared infrastructure
2. **Safety**: Blue-green deployment with instant rollback
3. **Cost Control**: Optimize task size, share resources where possible
4. **Maintainability**: Clean module structure, independent versioning

**Critical Success Factors**:
- Zero downtime during migration (Route53 weighted routing)
- Secure secret migration (never log private keys)
- Separate Terraform state (avoid production conflicts)
- Comprehensive testing before DNS cutover

**Timeline**: 4 weeks for complete migration (preparation → deployment → DNS cutover → decommission)

**Budget**: $59/month (current) → $90-110/month (standalone optimized)

---

**End of Report**
