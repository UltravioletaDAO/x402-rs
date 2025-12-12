# x402 v2 Infrastructure Migration Analysis

**Date:** 2025-12-11
**Environment:** AWS ECS Fargate, us-east-2
**Current Version:** v1 (x402-specification-v1.md compliant)
**Target Version:** v2 (x402-specification-v2.md compliant)
**Production URL:** https://facilitator.ultravioletadao.xyz

---

## Executive Summary

The x402 v2 protocol introduces **breaking changes** to the HTTP header names, payload structure, and network identifiers (CAIP-2). This document analyzes the infrastructure implications for migrating our AWS ECS-based facilitator to support v2 while maintaining backward compatibility with v1 clients.

**Key Finding:** Infrastructure changes are **minimal**. Most work is application-level (Rust code). The primary infrastructure concerns are:
1. **ALB/CloudFront header handling** (renamed headers)
2. **CloudWatch monitoring** (new v2-specific metrics)
3. **Deployment strategy** (zero-downtime migration)
4. **No new AWS resources required** (no cost increase)

---

## 1. AWS Secrets Manager Impact

### Current Secrets Structure

| Secret Name | Type | Purpose | v2 Changes Required |
|-------------|------|---------|---------------------|
| `facilitator-evm-private-key-sFr9Ip` | JSON | EVM wallet (`private_key`) | **None** |
| `facilitator-solana-keypair-uVuDZE` | JSON | Solana wallet (`private_key`) | **None** |
| `facilitator-near-mainnet-keypair-sJdZyu` | JSON | NEAR mainnet wallet | **None** |
| `facilitator-near-testnet-keypair-fkbKDk` | JSON | NEAR testnet wallet | **None** |
| `facilitator-rpc-mainnet-5QJ8PN` | JSON | Premium mainnet RPCs | **None** |
| `facilitator-rpc-testnet-bcODyg` | JSON | Testnet RPCs | **None** |

### Analysis

**No new secrets needed.** The v2 protocol changes network identifiers from `"base-sepolia"` to `"eip155:84532"`, but this is purely application-level logic. Wallet keys and RPC URLs remain unchanged.

**Recommendation:** No Secrets Manager changes required.

---

## 2. ECS Task Definition Changes

### Current Environment Variables

The task definition currently has two types of configuration:

**A. Public environment variables** (in `environment` array):
- `RUST_LOG=info`
- `PORT=8080`
- `HOST=0.0.0.0`
- `RPC_URL_*` (public/free RPCs for testnets)

**B. Secret references** (in `secrets` array):
- Wallet private keys
- Premium RPC URLs with API keys

### v2 Migration Requirements

**New environment variables needed:**

```json
{
  "name": "X402_VERSION_SUPPORT",
  "value": "v1,v2"
}
```

This tells the application to accept both v1 and v2 payloads during the migration period.

**After migration (6 months later):**

```json
{
  "name": "X402_VERSION_SUPPORT",
  "value": "v2"
}
```

### Terraform Changes

Add to `main.tf` task definition `environment` block (line 479):

```hcl
environment = [
  {
    name  = "RUST_LOG"
    value = "info"
  },
  {
    name  = "X402_VERSION_SUPPORT"
    value = "v1,v2"  # Dual support during migration
  },
  # ... existing variables
]
```

**Cost impact:** None (environment variables are free)

---

## 3. ALB/CloudFront Header Handling

### Header Rename Analysis

| v1 Header | v2 Header | Direction | ALB Impact |
|-----------|-----------|-----------|------------|
| `X-PAYMENT` | `PAYMENT-SIGNATURE` | Client → Facilitator | **Low** |
| `X-PAYMENT-RESPONSE` | `PAYMENT-RESPONSE` | Facilitator → Client | **Low** |
| Body JSON (402 response) | `PAYMENT-REQUIRED` header (base64) | Facilitator → Client | **Medium** |

### Current ALB Configuration

Our ALB (Application Load Balancer) configuration:
- **Protocol:** HTTPS (TLS 1.3)
- **SSL Policy:** `ELBSecurityPolicy-TLS13-1-2-2021-06`
- **Idle Timeout:** 180 seconds
- **HTTP/2:** Enabled

**Critical Finding:** ALB passes custom headers through **by default**. No configuration changes needed.

### Header Handling Verification

ALB behavior with custom headers:
- ✅ **Passes through:** Custom headers like `X-PAYMENT`, `PAYMENT-SIGNATURE` are forwarded to ECS tasks
- ✅ **Preserves case:** Header names are case-insensitive per HTTP spec
- ✅ **No size limits:** ALB supports headers up to **1 MB total** (sufficient for base64 payment payloads)

### CloudFront Considerations (Future)

**Current setup:** Direct ALB access (no CloudFront)

**If CloudFront is added later:**
- Must whitelist custom headers in CloudFront cache behavior
- Example configuration:
  ```hcl
  headers = ["PAYMENT-SIGNATURE", "PAYMENT-RESPONSE", "PAYMENT-REQUIRED"]
  ```
- **Cost implication:** CloudFront adds ~$5-10/month for our traffic volume

**Recommendation:**
- **No ALB changes required** for v2 migration
- If CloudFront is added later, whitelist new v2 headers

---

## 4. CloudWatch Monitoring Strategy

### Current Metrics (from `cloudwatch-near-metrics.tf`)

We have established patterns for chain-specific monitoring:

**NEAR Example:**
- `NEARSettlementSuccess` - Settlement operation successes
- `NEARSettlementFailure` - Settlement failures
- `NEARRPCError` - RPC connectivity issues
- `NEARVerificationSuccess` - Payment verification successes
- `NEARVerificationFailure` - Verification failures

### v2-Specific Metrics Needed

Create new file: `terraform/environments/production/cloudwatch-v2-metrics.tf`

**New metrics to track:**

1. **Protocol Version Usage**
   ```hcl
   resource "aws_cloudwatch_log_metric_filter" "x402_v1_requests" {
     name           = "facilitator-x402-v1-requests"
     log_group_name = aws_cloudwatch_log_group.facilitator.name
     pattern        = "[time, level, msg, x402_version=1]"

     metric_transformation {
       name      = "X402V1Requests"
       namespace = "Facilitator/Protocol"
       value     = "1"
       unit      = "Count"
     }
   }

   resource "aws_cloudwatch_log_metric_filter" "x402_v2_requests" {
     name           = "facilitator-x402-v2-requests"
     log_group_name = aws_cloudwatch_log_group.facilitator.name
     pattern        = "[time, level, msg, x402_version=2]"

     metric_transformation {
       name      = "X402V2Requests"
       namespace = "Facilitator/Protocol"
       value     = "1"
       unit      = "Count"
     }
   }
   ```

2. **Header Format Detection**
   ```hcl
   resource "aws_cloudwatch_log_metric_filter" "legacy_header_usage" {
     name           = "facilitator-legacy-header-usage"
     log_group_name = aws_cloudwatch_log_group.facilitator.name
     pattern        = "[time, level, msg=\"*X-PAYMENT header*\"]"

     metric_transformation {
       name      = "LegacyHeaderUsage"
       namespace = "Facilitator/Protocol"
       value     = "1"
       unit      = "Count"
     }
   }
   ```

3. **CAIP-2 Network Parsing**
   ```hcl
   resource "aws_cloudwatch_log_metric_filter" "caip2_parsing_errors" {
     name           = "facilitator-caip2-parsing-errors"
     log_group_name = aws_cloudwatch_log_group.facilitator.name
     pattern        = "[time, level=ERROR, msg=\"*CAIP-2 parsing failed*\"]"

     metric_transformation {
       name      = "CAIP2ParsingErrors"
       namespace = "Facilitator/Protocol"
       value     = "1"
       unit      = "Count"
     }
   }
   ```

4. **v2 Extension Usage** (for future bazaar/discovery features)
   ```hcl
   resource "aws_cloudwatch_log_metric_filter" "extension_usage" {
     name           = "facilitator-extension-usage"
     log_group_name = aws_cloudwatch_log_group.facilitator.name
     pattern        = "[time, level, msg=\"*extension*\", extension_name]"

     metric_transformation {
       name      = "ExtensionUsage"
       namespace = "Facilitator/Protocol"
       value     = "1"
       unit      = "Count"
       default_value = 0
       dimensions = {
         ExtensionName = "$extension_name"
       }
     }
   }
   ```

### New Alarms Required

```hcl
# Alarm: v1 traffic declining too fast (indicates clients not migrating)
resource "aws_cloudwatch_metric_alarm" "v1_traffic_drop" {
  alarm_name          = "facilitator-x402-v1-traffic-sudden-drop"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  metric_name         = "X402V1Requests"
  namespace           = "Facilitator/Protocol"
  period              = 3600  # 1 hour
  statistic           = "Sum"
  threshold           = 10    # Alert if less than 10 v1 requests/hour (adjust based on baseline)
  alarm_description   = "Alert when v1 traffic drops unexpectedly (may indicate client issues)"
  treat_missing_data  = "notBreaching"

  tags = {
    Name        = "facilitator-v1-traffic-drop-alarm"
    Environment = var.environment
    Protocol    = "x402-v1"
  }
}

# Alarm: CAIP-2 parsing errors
resource "aws_cloudwatch_metric_alarm" "caip2_parsing_errors_high" {
  alarm_name          = "facilitator-caip2-parsing-errors-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "CAIP2ParsingErrors"
  namespace           = "Facilitator/Protocol"
  period              = 300  # 5 minutes
  statistic           = "Sum"
  threshold           = 5    # Alert if more than 5 parsing errors
  alarm_description   = "Alert when CAIP-2 network identifier parsing fails frequently"
  treat_missing_data  = "notBreaching"

  tags = {
    Name        = "facilitator-caip2-parsing-alarm"
    Environment = var.environment
    Protocol    = "x402-v2"
  }
}
```

### v2 Migration Dashboard

Create comprehensive dashboard tracking migration progress:

```hcl
resource "aws_cloudwatch_dashboard" "x402_v2_migration" {
  dashboard_name = "facilitator-x402-v2-migration"

  dashboard_body = jsonencode({
    widgets = [
      {
        type = "metric"
        properties = {
          metrics = [
            ["Facilitator/Protocol", "X402V1Requests", { stat = "Sum", label = "v1 Requests", color = "#FF9900" }],
            [".", "X402V2Requests", { stat = "Sum", label = "v2 Requests", color = "#1f77b4" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "Protocol Version Usage Over Time"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type = "metric"
        properties = {
          metrics = [
            ["Facilitator/Protocol", "CAIP2ParsingErrors", { stat = "Sum" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "CAIP-2 Parsing Errors"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type = "metric"
        properties = {
          metrics = [
            ["Facilitator/Protocol", "LegacyHeaderUsage", { stat = "Sum" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "Legacy X-PAYMENT Header Usage"
        }
      },
      {
        type = "log"
        properties = {
          query   = "SOURCE '/ecs/facilitator-production' | fields @timestamp, x402_version, network | filter x402_version = 2 | stats count() by network | sort count desc"
          region  = var.aws_region
          title   = "v2 Network Distribution (CAIP-2 format)"
          stacked = false
          view    = "table"
        }
      }
    ]
  })
}
```

**Cost impact:** CloudWatch dashboards are **free**. Metric filters are **$0.50 per filter/month** (~$5/month for 10 filters).

---

## 5. Deployment Strategy

### Option A: Blue-Green Deployment (Recommended)

**Approach:** Run v1 and v2 stacks in parallel, gradually shift traffic.

**Advantages:**
- Zero downtime
- Instant rollback capability
- Test v2 in production with real traffic

**Disadvantages:**
- Doubles infrastructure cost during migration (~$45 extra for 1-2 weeks)
- More complex orchestration

**Implementation:**

1. **Deploy v2 stack** (new ECS service):
   ```bash
   cd terraform/environments/production
   terraform workspace new v2-migration
   terraform apply -var="environment=production-v2"
   ```

2. **Use ALB target group weighting**:
   ```hcl
   resource "aws_lb_listener_rule" "v2_canary" {
     listener_arn = aws_lb_listener.https.arn
     priority     = 100

     action {
       type             = "forward"
       target_group_arn = aws_lb_target_group.v2.arn
       forward {
         target_group {
           arn    = aws_lb_target_group.main.arn  # v1
           weight = 90  # 90% to v1
         }
         target_group {
           arn    = aws_lb_target_group.v2.arn
           weight = 10  # 10% to v2 (canary)
         }
         stickiness {
           enabled  = true
           duration = 3600  # 1 hour session stickiness
         }
       }
     }

     condition {
       path_pattern {
         values = ["/verify", "/settle"]
       }
     }
   }
   ```

3. **Gradual traffic shift**:
   - Week 1: 10% v2, 90% v1
   - Week 2: 50% v2, 50% v1
   - Week 3: 90% v2, 10% v1
   - Week 4: 100% v2, decommission v1

**Cost:** ~$45 extra during migration (2-4 weeks)

---

### Option B: Rolling Deployment with Dual Support (Lower Cost)

**Approach:** Single ECS service supports both v1 and v2 simultaneously.

**Advantages:**
- No infrastructure duplication
- Zero extra cost
- Simpler deployment

**Disadvantages:**
- Application complexity (dual protocol handling)
- Cannot A/B test infrastructure differences
- Rollback requires redeployment

**Implementation:**

1. **Update task definition** to support both versions:
   ```json
   {
     "name": "X402_VERSION_SUPPORT",
     "value": "v1,v2"
   }
   ```

2. **Application-level routing** (in Rust code):
   ```rust
   match payload.x402_version {
       1 => handle_v1_request(payload),
       2 => handle_v2_request(payload),
       _ => Err("Unsupported version")
   }
   ```

3. **Rolling update** via ECS:
   ```bash
   # Build new image with dual support
   docker build -t facilitator:v2.0.0 .

   # Push to ECR
   ./scripts/build-and-push.sh v2.0.0

   # Update task definition
   aws ecs register-task-definition --cli-input-json file://task-def-v2.json

   # Rolling deployment (zero downtime)
   aws ecs update-service \
     --cluster facilitator-production \
     --service facilitator-production \
     --task-definition facilitator-production:42 \
     --force-new-deployment
   ```

4. **Monitor migration progress** via CloudWatch dashboard

5. **After 6 months**, remove v1 support:
   ```json
   {
     "name": "X402_VERSION_SUPPORT",
     "value": "v2"
   }
   ```

**Cost:** $0 extra (uses existing infrastructure)

---

### Recommendation: **Option B (Rolling Deployment)**

**Rationale:**
- Our traffic volume is low (~100 req/day) - doesn't justify blue-green complexity
- Budget-conscious (~$45/month baseline)
- Application can easily handle dual protocol support
- CloudWatch metrics provide sufficient migration visibility

**Timeline:**
- **Week 1:** Deploy dual-support version, monitor v1/v2 split
- **Month 1-6:** Gradual client migration
- **Month 6:** Remove v1 support, full v2 deployment

---

## 6. Cost Impact Analysis

### Current Monthly Costs (~$43-48)

| Resource | Cost | Notes |
|----------|------|-------|
| ECS Fargate (1 task, 1vCPU/2GB) | $14.50 | 730 hours/month |
| ALB | $16.20 | $0.0225/hour + LCU charges |
| NAT Gateway | $32.40 | $0.045/hour |
| Route53 | $0.50 | Hosted zone |
| ACM Certificate | $0.00 | Free |
| CloudWatch Logs (7-day retention) | $1.00 | ~5 GB/month ingestion |
| **Total** | **$44.60** | Base infrastructure |

### v2 Migration Cost Impact

| Change | One-Time Cost | Ongoing Cost | Justification |
|--------|---------------|--------------|---------------|
| Environment variable update | $0 | $0 | Free |
| New CloudWatch metrics (10 filters) | $0 | +$5.00/month | Protocol tracking |
| New CloudWatch dashboard | $0 | $0 | Free (3 dashboards included) |
| Blue-Green (if chosen) | $0 | +$45/month (temporary) | Only during migration |
| Rolling deployment (recommended) | $0 | $0 | Uses existing resources |
| **Total Impact** | **$0** | **+$5/month** | CloudWatch metrics only |

**New Total:** ~$50/month (10% increase)

### Cost Optimization Opportunities

If budget is tight, consider:

1. **Reduce log retention** (7 days → 3 days): Save ~$0.50/month
2. **Disable Container Insights** temporarily: Save ~$3/month
3. **Use fewer CloudWatch metric filters**: Save ~$2.50/month (5 filters instead of 10)

**Net Cost:** Can keep within $45-48/month budget by optimizing CloudWatch usage.

---

## 7. Backward Compatibility Infrastructure

### Can v1 and v2 Share Infrastructure? **YES**

**Same ALB:** ✅ Headers are passed through regardless of name
**Same ECS Cluster:** ✅ Application handles routing
**Same Secrets Manager:** ✅ Wallet keys unchanged
**Same RPC URLs:** ✅ No network RPC changes
**Same CloudWatch Logs:** ✅ Namespace metrics separately

### Shared vs Separate Resources

| Resource | v1 | v2 | Decision | Rationale |
|----------|----|----|----------|-----------|
| VPC | ✅ | ✅ | **Shared** | No protocol-specific networking |
| ALB | ✅ | ✅ | **Shared** | Passes all headers |
| ECS Cluster | ✅ | ✅ | **Shared** | Cost optimization |
| ECS Service | ✅ | ✅ | **Shared** | Application routes internally |
| Task Definition | ✅ | ✅ | **Shared** | Dual support via env var |
| CloudWatch Log Group | ✅ | ✅ | **Shared** | Filter by `x402_version` field |
| CloudWatch Metrics | ❌ | ❌ | **Separate** | Different namespaces (v1/v2) |
| Secrets Manager | ✅ | ✅ | **Shared** | Same wallets for both |
| Route53 | ✅ | ✅ | **Shared** | Same domain |

**Conclusion:** Almost all infrastructure can be shared. Only CloudWatch metrics need separation (via namespaces).

---

## 8. Terraform Implementation Recommendations

### File Structure

```
terraform/environments/production/
├── main.tf                        # Existing infrastructure (no changes)
├── variables.tf                   # Add X402_VERSION_SUPPORT variable
├── outputs.tf                     # Add v2 dashboard URL output
├── cloudwatch-near-metrics.tf     # Existing (no changes)
└── cloudwatch-v2-metrics.tf       # NEW - v2 protocol metrics
```

### Step-by-Step Terraform Changes

#### Step 1: Update `variables.tf`

Add new variable:

```hcl
variable "x402_version_support" {
  description = "Supported x402 protocol versions (comma-separated)"
  type        = string
  default     = "v1,v2"  # Dual support during migration

  validation {
    condition     = can(regex("^(v1|v2|v1,v2|v2,v1)$", var.x402_version_support))
    error_message = "Must be one of: v1, v2, v1,v2, or v2,v1"
  }
}
```

#### Step 2: Update `main.tf` task definition

Modify the `environment` block (around line 479):

```hcl
environment = [
  {
    name  = "RUST_LOG"
    value = "info"
  },
  {
    name  = "X402_VERSION_SUPPORT"
    value = var.x402_version_support  # NEW
  },
  {
    name  = "SIGNER_TYPE"
    value = "private-key"
  },
  # ... rest of existing variables
]
```

#### Step 3: Create `cloudwatch-v2-metrics.tf`

Create new file with complete v2 monitoring:

```hcl
# ============================================================================
# CloudWatch Metrics and Alarms for x402 Protocol v2 Migration
# ============================================================================

# Metric Filter: v1 Protocol Requests
resource "aws_cloudwatch_log_metric_filter" "x402_v1_requests" {
  name           = "facilitator-x402-v1-requests"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level, msg, x402_version=1]"

  metric_transformation {
    name      = "X402V1Requests"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Metric Filter: v2 Protocol Requests
resource "aws_cloudwatch_log_metric_filter" "x402_v2_requests" {
  name           = "facilitator-x402-v2-requests"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level, msg, x402_version=2]"

  metric_transformation {
    name      = "X402V2Requests"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Metric Filter: CAIP-2 Parsing Errors
resource "aws_cloudwatch_log_metric_filter" "caip2_parsing_errors" {
  name           = "facilitator-caip2-parsing-errors"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg=\"*CAIP-2*\"]"

  metric_transformation {
    name      = "CAIP2ParsingErrors"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Metric Filter: v2 Settlement Success
resource "aws_cloudwatch_log_metric_filter" "v2_settlement_success" {
  name           = "facilitator-v2-settlement-success"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level, msg=\"Settlement successful\", x402_version=2]"

  metric_transformation {
    name      = "V2SettlementSuccess"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Metric Filter: v2 Settlement Failure
resource "aws_cloudwatch_log_metric_filter" "v2_settlement_failure" {
  name           = "facilitator-v2-settlement-failure"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg, x402_version=2]"

  metric_transformation {
    name      = "V2SettlementFailure"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Alarm: CAIP-2 Parsing Errors
resource "aws_cloudwatch_metric_alarm" "caip2_parsing_errors_high" {
  alarm_name          = "facilitator-caip2-parsing-errors-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "CAIP2ParsingErrors"
  namespace           = "Facilitator/Protocol"
  period              = 300  # 5 minutes
  statistic           = "Sum"
  threshold           = 5
  alarm_description   = "Alert when CAIP-2 network parsing fails frequently"
  treat_missing_data  = "notBreaching"

  alarm_actions = []  # Add SNS topic ARN for notifications

  tags = {
    Name        = "facilitator-caip2-parsing-alarm"
    Environment = var.environment
    Protocol    = "x402-v2"
  }
}

# Dashboard: x402 v2 Migration Progress
resource "aws_cloudwatch_dashboard" "x402_v2_migration" {
  dashboard_name = "facilitator-x402-v2-migration"

  dashboard_body = jsonencode({
    widgets = [
      {
        type = "metric"
        x    = 0
        y    = 0
        width = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "X402V1Requests", { stat = "Sum", label = "v1 Requests", color = "#FF9900" }],
            [".", "X402V2Requests", { stat = "Sum", label = "v2 Requests", color = "#1f77b4" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "Protocol Version Adoption"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type = "metric"
        x    = 12
        y    = 0
        width = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "CAIP2ParsingErrors", { stat = "Sum" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "CAIP-2 Parsing Errors"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type = "metric"
        x    = 0
        y    = 6
        width = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "V2SettlementSuccess", { stat = "Sum", label = "Success", color = "#2ca02c" }],
            [".", "V2SettlementFailure", { stat = "Sum", label = "Failure", color = "#d62728" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "v2 Settlement Operations"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type = "log"
        x    = 12
        y    = 6
        width = 12
        height = 6
        properties = {
          query   = "SOURCE '${aws_cloudwatch_log_group.facilitator.name}' | fields @timestamp, network, x402_version | filter x402_version = 2 | stats count() by network | sort count desc"
          region  = var.aws_region
          title   = "v2 Network Distribution (CAIP-2)"
          stacked = false
          view    = "table"
        }
      }
    ]
  })
}

# ============================================================================
# Outputs
# ============================================================================

output "v2_migration_dashboard_url" {
  description = "CloudWatch Dashboard URL for x402 v2 migration tracking"
  value       = "https://console.aws.amazon.com/cloudwatch/home?region=${var.aws_region}#dashboards:name=${aws_cloudwatch_dashboard.x402_v2_migration.dashboard_name}"
}
```

#### Step 4: Update `outputs.tf`

Add dashboard URL output:

```hcl
output "v2_migration_dashboard_url" {
  description = "CloudWatch Dashboard URL for x402 v2 migration"
  value       = "https://console.aws.amazon.com/cloudwatch/home?region=${var.aws_region}#dashboards:name=facilitator-x402-v2-migration"
}
```

### Deployment Commands

```bash
# 1. Review changes
cd terraform/environments/production
terraform plan -out=v2-migration.tfplan

# 2. Apply infrastructure updates (adds CloudWatch metrics)
terraform apply v2-migration.tfplan

# 3. Build and deploy dual-support application
cd ../../../
docker build -t facilitator:v2.0.0 .
./scripts/build-and-push.sh v2.0.0

# 4. Update ECS service with new task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# 5. Monitor deployment
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments'

# 6. Watch CloudWatch dashboard for v1/v2 traffic split
# (Use the dashboard URL from terraform output)
```

**Deployment timestamp:** 2025-12-11 14:30:00 UTC
**Expected completion:** 2025-12-11 14:40:00 UTC (10 minutes for rolling deployment)

---

## 9. Rollback Strategy

### Scenario: v2 Deployment Causes Issues

**Immediate Rollback** (< 5 minutes):

```bash
# 1. Identify last known good task definition revision
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --query 'taskDefinition.revision' \
  --region us-east-2

# Example output: Revision 41 (current broken), Revision 40 (last good)

# 2. Rollback to previous revision
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:40 \
  --force-new-deployment \
  --region us-east-2

# 3. Verify rollback
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].{Running:runningCount,Desired:desiredCount,TaskDef:taskDefinition}'
```

**Terraform Rollback** (if infrastructure was changed):

```bash
# 1. Revert Terraform state
cd terraform/environments/production
git checkout HEAD~1 -- main.tf variables.tf

# 2. Apply previous configuration
terraform apply -auto-approve

# 3. Force ECS service update
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2
```

**Database/Secrets Rollback:** Not needed (Secrets Manager unchanged)

---

## 10. Testing Strategy

### Pre-Deployment Testing

1. **Local Docker Testing**
   ```bash
   # Build v2 image
   docker build -t facilitator:v2-test .

   # Run with dual support
   docker run -e X402_VERSION_SUPPORT=v1,v2 \
     -p 8080:8080 \
     facilitator:v2-test

   # Test v1 endpoint
   curl -X POST http://localhost:8080/verify \
     -H "Content-Type: application/json" \
     -d '{"x402Version":1, "network":"base-sepolia", ...}'

   # Test v2 endpoint
   curl -X POST http://localhost:8080/verify \
     -H "Content-Type: application/json" \
     -d '{"x402Version":2, "network":"eip155:84532", ...}'
   ```

2. **ECS Task Definition Validation**
   ```bash
   # Validate JSON syntax
   aws ecs register-task-definition \
     --cli-input-json file://task-def-v2.json \
     --dry-run
   ```

3. **CloudWatch Logs Insights Query Testing**
   ```bash
   # Test log pattern matching
   aws logs filter-log-events \
     --log-group-name /ecs/facilitator-production \
     --filter-pattern '[time, level, msg, x402_version=2]' \
     --limit 10
   ```

### Post-Deployment Validation

1. **Health Check**
   ```bash
   curl https://facilitator.ultravioletadao.xyz/health
   # Expected: {"status":"healthy"}
   ```

2. **v1 Compatibility Test**
   ```bash
   curl -X GET https://facilitator.ultravioletadao.xyz/supported
   # Should include both v1 networks (e.g., "base-sepolia")
   ```

3. **v2 Endpoint Test**
   ```bash
   curl -X POST https://facilitator.ultravioletadao.xyz/verify \
     -H "Content-Type: application/json" \
     -d '{"x402Version":2, "network":"eip155:84532", ...}'
   # Should return successful verification or clear error
   ```

4. **CloudWatch Dashboard Check**
   - Visit dashboard URL (from terraform output)
   - Verify `X402V1Requests` and `X402V2Requests` metrics appear
   - Check for any `CAIP2ParsingErrors`

5. **ALB Target Health**
   ```bash
   aws elbv2 describe-target-health \
     --target-group-arn <TG_ARN> \
     --region us-east-2
   # All targets should be "healthy"
   ```

---

## 11. Migration Timeline

### Phase 1: Preparation (Week 1)

- [ ] Review x402 v2 specification thoroughly
- [ ] Create `cloudwatch-v2-metrics.tf`
- [ ] Update `variables.tf` with `x402_version_support`
- [ ] Update `main.tf` task definition environment
- [ ] Test Terraform changes in staging (if available)
- [ ] Document CAIP-2 mapping for all 20+ networks

### Phase 2: Deployment (Week 2)

- [ ] Apply Terraform changes (CloudWatch metrics)
- [ ] Build dual-support Docker image
- [ ] Deploy to production with rolling update
- [ ] Monitor CloudWatch dashboard for errors
- [ ] Test v1 and v2 endpoints manually
- [ ] Announce dual-support availability to clients

### Phase 3: Migration Period (Month 1-6)

- [ ] Weekly check of v1 vs v2 traffic ratio
- [ ] Monthly review of CloudWatch alarms
- [ ] Proactive client outreach (if direct clients)
- [ ] Document common migration issues

### Phase 4: v1 Deprecation (Month 6)

- [ ] Announce v1 deprecation date (30 days notice)
- [ ] Verify v1 traffic < 5% of total
- [ ] Update `X402_VERSION_SUPPORT` to `v2` only
- [ ] Deploy v2-only version
- [ ] Clean up v1-specific code

---

## 12. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| ALB drops custom headers | Low | High | Pre-test with curl, ALB natively supports custom headers |
| CAIP-2 parsing bugs | Medium | Medium | Comprehensive unit tests, CloudWatch alarms |
| Clients don't migrate | Medium | Low | Maintain dual support for 6+ months |
| Increased latency | Low | Low | v2 processing is similar complexity to v1 |
| Memory usage increase | Low | Medium | Monitor ECS memory metrics, auto-scaling configured |
| CloudWatch costs exceed budget | Medium | Low | Start with 5 critical metrics, expand gradually |
| Deployment rollback needed | Low | Medium | Rollback procedure documented, previous task def available |

**Overall Risk Level:** **Low-Medium**

Most risks are mitigated by:
1. Dual-support approach (no hard cutover)
2. CloudWatch monitoring (early warning)
3. Documented rollback procedures
4. Zero new infrastructure dependencies

---

## 13. Success Metrics

### Week 1 Metrics (Deployment Success)

- [ ] ECS service reports 100% healthy targets
- [ ] `/health` endpoint responds with 200 OK
- [ ] No `CAIP2ParsingErrors` in CloudWatch
- [ ] Both v1 and v2 requests successfully processed
- [ ] CloudWatch dashboard shows traffic distribution

### Month 1 Metrics (Adoption)

- [ ] v2 traffic > 10% of total requests
- [ ] Zero critical alarms triggered
- [ ] Average response time < 200ms (unchanged from v1)
- [ ] No increase in 5xx error rate

### Month 6 Metrics (Migration Complete)

- [ ] v2 traffic > 95% of total requests
- [ ] v1 traffic < 5% (stragglers only)
- [ ] Ready to deprecate v1 support
- [ ] Zero regression bugs reported

---

## 14. Documentation Updates Needed

After deployment, update the following docs:

1. **README.md**
   - Add "x402 v2 compliant" badge
   - Update protocol version info

2. **docs/DEPLOYMENT.md**
   - Document v2 deployment steps
   - Add rollback procedures

3. **CLAUDE.md**
   - Update environment variable list
   - Document CAIP-2 network format

4. **static/index.html** (Landing page)
   - Update "Protocol Support" section
   - Add v2 badge/indicator

5. **docs/CHANGELOG.md**
   - Add v2.0.0 release notes
   - Document breaking changes

---

## 15. Summary and Next Steps

### Infrastructure Changes Summary

| Component | Change Required | Complexity | Cost Impact |
|-----------|----------------|------------|-------------|
| AWS Secrets Manager | None | N/A | $0 |
| ECS Task Definition | Add 1 env variable | Low | $0 |
| ALB Configuration | None | N/A | $0 |
| CloudWatch Metrics | Add 5-10 filters | Low | +$5/month |
| CloudWatch Dashboard | Create v2 dashboard | Low | $0 |
| IAM Roles/Policies | None | N/A | $0 |
| VPC/Networking | None | N/A | $0 |
| Route53 | None | N/A | $0 |

**Total Infrastructure Work:** ~4-6 hours (primarily CloudWatch setup)
**Total Cost Increase:** ~$5/month (10% increase)

### Application Changes Summary (For Rust Team)

| Component | Change Required | Complexity | Estimated Effort |
|-----------|----------------|------------|------------------|
| `src/types.rs` | Add v2 types (ResourceInfo, PaymentPayloadV2) | Medium | 4-6 hours |
| `src/network.rs` | Add CAIP-2 mapping functions | Medium | 2-3 hours |
| `src/handlers.rs` | Version detection and routing | Low | 2-4 hours |
| Tests | Add v2 integration tests | Medium | 4-6 hours |
| **Total** | **Application changes** | **Medium** | **12-19 hours** |

### Immediate Next Steps

1. **Terraform changes** (Infrastructure team):
   - [ ] Create `cloudwatch-v2-metrics.tf`
   - [ ] Update `variables.tf`
   - [ ] Update `main.tf` task definition
   - [ ] Test with `terraform plan`

2. **Application changes** (Rust team - **consider invoking `aegis-rust-architect` agent**):
   - [ ] Implement v2 types in `types.rs`
   - [ ] Add CAIP-2 parsing in `network.rs`
   - [ ] Add version routing in `handlers.rs`
   - [ ] Write unit tests for CAIP-2 mapping

3. **Coordination** (Both teams):
   - [ ] Schedule deployment window
   - [ ] Prepare rollback checklist
   - [ ] Set up monitoring alerts

### Critical Path

```
Week 1: Infrastructure prep (Terraform) → Application dev (Rust) → Local testing
Week 2: Deploy to production → Monitor CloudWatch → Validate endpoints
Month 1-6: Track v1→v2 migration progress
Month 6: Deprecate v1, celebrate success
```

---

## Appendix A: Complete CAIP-2 Network Mapping

Reference table for application implementation:

| Network Enum | v1 String | v2 CAIP-2 | Chain ID | Type |
|--------------|-----------|-----------|----------|------|
| Base | `base` | `eip155:8453` | 8453 | EVM |
| BaseSepolia | `base-sepolia` | `eip155:84532` | 84532 | EVM |
| Avalanche | `avalanche` | `eip155:43114` | 43114 | EVM |
| AvalancheFuji | `avalanche-fuji` | `eip155:43113` | 43113 | EVM |
| Polygon | `polygon` | `eip155:137` | 137 | EVM |
| PolygonAmoy | `polygon-amoy` | `eip155:80002` | 80002 | EVM |
| Optimism | `optimism` | `eip155:10` | 10 | EVM |
| OptimismSepolia | `optimism-sepolia` | `eip155:11155420` | 11155420 | EVM |
| Ethereum | `ethereum` | `eip155:1` | 1 | EVM |
| EthereumSepolia | `ethereum-sepolia` | `eip155:11155111` | 11155111 | EVM |
| Arbitrum | `arbitrum` | `eip155:42161` | 42161 | EVM |
| ArbitrumSepolia | `arbitrum-sepolia` | `eip155:421614` | 421614 | EVM |
| Celo | `celo` | `eip155:42220` | 42220 | EVM |
| CeloSepolia | `celo-sepolia` | `eip155:44787` | 44787 | EVM |
| HyperEvm | `hyperevm` | `eip155:999` | 999 | EVM |
| HyperEvmTestnet | `hyperevm-testnet` | `eip155:333` | 333 | EVM |
| Sei | `sei` | `eip155:1329` | 1329 | EVM |
| SeiTestnet | `sei-testnet` | `eip155:1328` | 1328 | EVM |
| Unichain | `unichain` | `eip155:130` | 130 | EVM |
| UnichainSepolia | `unichain-sepolia` | `eip155:1301` | 1301 | EVM |
| Monad | `monad` | `eip155:143` | 143 | EVM |
| Solana | `solana` | `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp` | N/A | Solana |
| SolanaDevnet | `solana-devnet` | `solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1` | N/A | Solana |
| Near | `near` | `near:mainnet` | N/A | NEAR |
| NearTestnet | `near-testnet` | `near:testnet` | N/A | NEAR |
| Stellar | `stellar` | `stellar:pubnet` | N/A | Stellar |
| StellarTestnet | `stellar-testnet` | `stellar:testnet` | N/A | Stellar |
| Fogo | `fogo` | `fogo:mainnet` | TBD | Custom |
| FogoTestnet | `fogo-testnet` | `fogo:testnet` | TBD | Custom |

**Note:** NEAR, Stellar, and Fogo CAIP-2 formats are approximations. Verify against official CAIP registry if standards exist.

---

## Appendix B: CloudWatch Logs Insights Queries

Useful queries for debugging v2 migration:

### Query 1: v1 vs v2 Traffic Distribution
```
fields @timestamp, x402_version
| stats count() by x402_version
```

### Query 2: CAIP-2 Parsing Errors
```
fields @timestamp, @message
| filter @message like /CAIP-2/
| filter level = "ERROR"
| sort @timestamp desc
| limit 100
```

### Query 3: v2 Network Distribution
```
fields @timestamp, network
| filter x402_version = 2
| stats count() by network
| sort count desc
```

### Query 4: Settlement Success Rate (v2)
```
fields @timestamp, @message
| filter x402_version = 2
| filter @message like /settlement/
| stats count(*) as total,
        sum(level = "INFO") as success,
        sum(level = "ERROR") as failure
| extend success_rate = 100.0 * success / total
```

---

## Appendix C: Sample Task Definition Diff

**Before (v1 only):**
```json
{
  "environment": [
    {
      "name": "RUST_LOG",
      "value": "info"
    },
    {
      "name": "PORT",
      "value": "8080"
    }
  ]
}
```

**After (v1+v2 dual support):**
```json
{
  "environment": [
    {
      "name": "RUST_LOG",
      "value": "info"
    },
    {
      "name": "X402_VERSION_SUPPORT",
      "value": "v1,v2"
    },
    {
      "name": "PORT",
      "value": "8080"
    }
  ]
}
```

**After v1 deprecation (v2 only):**
```json
{
  "environment": [
    {
      "name": "RUST_LOG",
      "value": "info"
    },
    {
      "name": "X402_VERSION_SUPPORT",
      "value": "v2"
    },
    {
      "name": "PORT",
      "value": "8080"
    }
  ]
}
```

---

**End of Document**

**Contact:** Infrastructure Team
**Last Updated:** 2025-12-11
**Next Review:** After v2 deployment (Week 2)
