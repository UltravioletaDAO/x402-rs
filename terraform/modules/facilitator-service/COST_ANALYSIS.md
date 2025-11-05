# ECS Fargate Cost Analysis

Detailed cost breakdown and optimization strategies for Karmacadabra agent deployment.

## Monthly Cost Breakdown

### Base Configuration (Recommended)

| Service | Configuration | Monthly Cost | Notes |
|---------|--------------|--------------|-------|
| **Fargate Spot** | 5 agents × 0.25 vCPU / 0.5GB × 730 hrs | **$25-40** | 70% savings vs on-demand |
| **Application Load Balancer** | 1 ALB + 5 target groups | **$16-18** | Fixed + data transfer |
| **NAT Gateway** | 1 NAT in single AZ + data transfer | **$32-35** | $32 fixed + ~$0.045/GB |
| **CloudWatch Logs** | 5 log groups × 7-day retention | **$5-8** | ~$0.50/GB ingested |
| **Container Insights** | 5 services with metrics | **$3-5** | Included in CloudWatch costs |
| **ECR Storage** | 5 repositories × ~500MB each | **$1-2** | $0.10/GB/month |
| **VPC Endpoints** | 5 interface endpoints | **$0** | Free tier / minimal usage |
| **EBS (none)** | Fargate uses ephemeral storage | **$0** | No persistent volumes |
| **Route53 (optional)** | Hosted zone + records | **$0.50** | Only if using custom domain |
| **Data Transfer** | Outbound to internet | **$5-10** | ~$0.09/GB after 100GB free |

### **TOTAL: $79-96 per month**

## Cost Comparison Scenarios

### Scenario 1: Current Recommendation (Fargate Spot)
- 5 agents, 0.25 vCPU / 0.5GB each, 24/7 uptime
- **Total: $79-96/month**
- **Per agent: $15.80-19.20/month**

### Scenario 2: Fargate On-Demand (No Spot)
- Same configuration, but using on-demand pricing
- Fargate cost: **$80-120/month** (vs $25-40 Spot)
- **Total: $134-188/month**
- **Extra cost: +$55-92/month (70% more expensive)**

### Scenario 3: Multi-AZ NAT (High Availability)
- Same as Scenario 1, but 2 NAT Gateways
- NAT cost: **$64-70/month** (vs $32-35 single)
- **Total: $111-128/month**
- **Extra cost: +$32/month**

### Scenario 4: Larger Task Sizes (0.5 vCPU / 1GB)
- Double the resources per task
- Fargate Spot cost: **$50-80/month**
- **Total: $104-141/month**
- **Extra cost: +$25-45/month**

### Scenario 5: Business Hours Only (9 AM - 6 PM, Mon-Fri)
- Tasks run 45 hours/week instead of 168 hours/week
- Fargate Spot cost: **$7-11/month** (73% savings)
- **Total: $61-70/month**
- **Savings: -$18-26/month**

### Scenario 6: Scale to Zero When Idle
- Manually scale to 0 tasks when not in use
- Fargate cost: **$0/month** (when scaled to zero)
- **Total: $48-58/month** (just infrastructure)
- **Savings: -$31-38/month**

## Detailed Cost Analysis

### Fargate Pricing (us-east-2)

#### On-Demand Pricing
- **vCPU**: $0.04048/hour
- **Memory**: $0.004445/GB/hour

#### Spot Pricing (70% savings)
- **vCPU**: $0.01214/hour
- **Memory**: $0.001334/GB/hour

#### Our Configuration (0.25 vCPU / 0.5GB)
```
Spot per task per hour:
  = (0.25 × $0.01214) + (0.5 × $0.001334)
  = $0.003035 + $0.000667
  = $0.003702/hour

Per month (730 hours):
  = $0.003702 × 730
  = $2.70/month per task

5 agents × 1 task each:
  = $2.70 × 5
  = $13.51/month (base Fargate cost)

With task churn, scaling events, etc:
  ≈ $25-40/month realistic estimate
```

### ALB Pricing

```
Fixed cost: $0.0225/hour
  = $0.0225 × 730 hours
  = $16.43/month

LCU (Load Balancer Capacity Unit):
  - New connections: 25/sec
  - Active connections: 3,000
  - Processed bytes: 1GB/hour
  - Rule evaluations: 1,000

Typical LCU cost for 5 agents with moderate traffic:
  ≈ $0-2/month (usually under 1 LCU)

Total ALB: $16-18/month
```

### NAT Gateway Pricing

```
Fixed cost: $0.045/hour
  = $0.045 × 730 hours
  = $32.85/month

Data processing:
  = $0.045/GB processed

Estimated data transfer (agents to blockchain, OpenAI):
  ≈ 5-10GB/month outbound
  = 5GB × $0.045
  = $0.23-0.45/month

Total NAT: $33-35/month (single gateway)
```

### CloudWatch Logs Pricing

```
Ingestion: $0.50/GB
Storage: $0.03/GB/month (7-day retention ≈ 25% of monthly)

Estimated log volume:
  5 agents × 100MB/day = 500MB/day = 15GB/month

Ingestion cost:
  = 15GB × $0.50
  = $7.50/month

Storage cost (7-day retention):
  = 3.5GB avg × $0.03
  = $0.11/month

Total CloudWatch Logs: $7.50-8/month

With log filtering and optimization:
  ≈ $5-6/month realistic
```

### ECR Pricing

```
Storage: $0.10/GB/month

Estimated storage:
  5 images × 500MB each = 2.5GB
  With old versions: 3-4GB total

Storage cost:
  = 3.5GB × $0.10
  = $0.35/month

With lifecycle policies keeping only 5 versions:
  = 2.5GB × $0.10
  = $0.25/month

Data transfer (pulls from same region): Free

Total ECR: $0.25-2/month
```

## Cost Optimization Strategies

### High Impact (Save $20-50/month)

#### 1. Use Fargate Spot (CRITICAL)
- **Savings**: $55-80/month
- **Implementation**: Set `use_fargate_spot = true`
- **Trade-off**: Tasks may be interrupted (rare), auto-restart
- **Risk**: Low (tasks restart automatically within 2 minutes)

#### 2. Single NAT Gateway
- **Savings**: $32/month
- **Implementation**: Set `single_nat_gateway = true`
- **Trade-off**: No HA for NAT (if AZ fails, no internet)
- **Risk**: Low (AZ failures are rare, agents can tolerate brief outages)

#### 3. Scheduled Scaling (Business Hours Only)
- **Savings**: $18-30/month
- **Implementation**: Use AWS Application Auto Scaling scheduled actions
- **Trade-off**: Agents only available during specific hours
- **Risk**: Medium (requires predictable usage patterns)

### Medium Impact (Save $5-20/month)

#### 4. Reduce Log Retention
- **Savings**: $5-10/month
- **Implementation**: Set `log_retention_days = 3` (from 7)
- **Trade-off**: Less historical data for debugging
- **Risk**: Low (can export to S3 for long-term storage)

#### 5. Optimize Task Size
- **Savings**: $10-20/month
- **Implementation**: Test with 0.25 vCPU / 0.5GB (minimum)
- **Trade-off**: May affect performance under load
- **Risk**: Medium (requires thorough testing)

#### 6. Use VPC Endpoints
- **Savings**: $3-5/month (reduces NAT data transfer)
- **Implementation**: Already included (`enable_vpc_endpoints = true`)
- **Trade-off**: None (small fixed cost, but saves data transfer)
- **Risk**: None

### Low Impact (Save $1-5/month)

#### 7. Disable Container Insights
- **Savings**: $2-3/month
- **Implementation**: Set `enable_container_insights = false`
- **Trade-off**: Lose deep metrics visibility
- **Risk**: High (loses critical observability)
- **Recommendation**: NOT recommended

#### 8. Aggressive ECR Lifecycle Policy
- **Savings**: $1-2/month
- **Implementation**: Keep only 3 image versions (from 5)
- **Trade-off**: Less rollback flexibility
- **Risk**: Low

#### 9. Disable ALB Access Logs
- **Savings**: $1-2/month
- **Implementation**: Already disabled (`enable_alb_access_logs = false`)
- **Trade-off**: No detailed request logs
- **Risk**: Low (can enable temporarily for debugging)

## Cost Monitoring

### Set Up Budget Alerts

```bash
aws budgets create-budget \
  --account-id $(aws sts get-caller-identity --query Account --output text) \
  --budget '{
    "BudgetName": "Karmacadabra-Monthly",
    "BudgetLimit": {
      "Amount": "100",
      "Unit": "USD"
    },
    "TimeUnit": "MONTHLY",
    "BudgetType": "COST"
  }' \
  --notifications-with-subscribers '[{
    "Notification": {
      "NotificationType": "ACTUAL",
      "ComparisonOperator": "GREATER_THAN",
      "Threshold": 80,
      "ThresholdType": "PERCENTAGE"
    },
    "Subscribers": [{
      "SubscriptionType": "EMAIL",
      "Address": "your-email@example.com"
    }]
  }]'
```

### Monitor Daily Costs

```bash
# Get cost for current month
aws ce get-cost-and-usage \
  --time-period Start=$(date -d "$(date +%Y-%m-01)" +%Y-%m-%d),End=$(date +%Y-%m-%d) \
  --granularity DAILY \
  --metrics BlendedCost \
  --filter file://<(echo '{
    "Tags": {
      "Key": "Project",
      "Values": ["Karmacadabra"]
    }
  }')
```

### CloudWatch Cost Metrics

Create a custom CloudWatch dashboard to track:
- Fargate task count × task size
- NAT Gateway data processed
- ALB active connections
- CloudWatch Logs ingestion

## Extreme Cost Optimization

If you need to get under $50/month:

### Configuration
```hcl
# terraform.tfvars
use_fargate_spot = true
single_nat_gateway = true
task_cpu = 256
task_memory = 512
desired_count_per_service = 1
autoscaling_max_capacity = 2  # Reduce from 3
log_retention_days = 3  # Reduce from 7
enable_container_insights = false  # Disable (not recommended)
```

### Scheduled Scaling
```bash
# Scale down to 0 at 6 PM weekdays
aws application-autoscaling put-scheduled-action \
  --service-namespace ecs \
  --scalable-dimension ecs:service:DesiredCount \
  --resource-id service/facilitator-production/facilitator-production \
  --scheduled-action-name scale-down-evening \
  --schedule "cron(0 18 * * MON-FRI *)" \
  --scalable-target-action MinCapacity=0,MaxCapacity=0 \
  --region us-east-2

# Scale up at 9 AM weekdays
aws application-autoscaling put-scheduled-action \
  --service-namespace ecs \
  --scalable-dimension ecs:service:DesiredCount \
  --resource-id service/facilitator-production/facilitator-production \
  --scheduled-action-name scale-up-morning \
  --schedule "cron(0 9 * * MON-FRI *)" \
  --scalable-target-action MinCapacity=1,MaxCapacity=2 \
  --region us-east-2
```

**Result**: ~$40-55/month (business hours only)

## Cost by Environment

### Development Environment
- 1-2 agents for testing
- Scaled to 0 when not in use
- Smaller task sizes
- **Cost**: $20-30/month

### Staging Environment
- 3-4 agents
- Business hours only (9-5, Mon-Fri)
- Medium task sizes
- **Cost**: $40-60/month

### Production Environment
- All 5 agents
- 24/7 uptime
- Auto-scaling enabled
- **Cost**: $79-96/month (current recommendation)

## ROI Analysis

### Cost per Agent
- **Monthly**: $15.80-19.20 per agent
- **Daily**: $0.53-0.64 per agent
- **Hourly**: $0.022-0.027 per agent

### Break-Even Analysis

If each agent processes:
- 100 transactions/day @ $0.01 GLUE each = $1/day revenue
- Monthly revenue per agent: $30
- Monthly cost per agent: $16-19
- **Profit per agent: $11-14/month**

For 5 agents:
- **Monthly revenue**: $150
- **Monthly cost**: $79-96
- **Monthly profit**: $54-71

## Conclusion

**Recommended Configuration for Cost Optimization**:
- ✅ Fargate Spot enabled (saves $55-80/month)
- ✅ Single NAT Gateway (saves $32/month)
- ✅ Smallest task sizes (0.25 vCPU / 0.5GB)
- ✅ Short log retention (7 days)
- ✅ Auto-scaling max 3 tasks
- ✅ VPC endpoints enabled

**Target Monthly Cost**: $79-96

**Further Savings Available**:
- Scheduled scaling (business hours): -$18-30/month
- Scale to zero when idle: -$31-38/month
- Reduce to 3 agents: -$30/month

**Not Recommended to Cut**:
- Container Insights (lose observability)
- Task execution role permissions (security risk)
- Health checks (reliability risk)
