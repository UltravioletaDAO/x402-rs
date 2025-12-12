# ============================================================================
# CloudWatch Metrics and Alarms for x402 Protocol v2 Migration
# ============================================================================
# Created: 2025-12-11
# Purpose: Track x402 protocol version adoption and v2-specific metrics
#
# This file adds monitoring for:
# - v1 vs v2 protocol usage
# - CAIP-2 network identifier parsing
# - v2 settlement operations
# - Migration progress dashboard
#
# Cost Impact: ~$5/month (CloudWatch metric filters)

# ============================================================================
# Metric Filters - Protocol Version Tracking
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

# ============================================================================
# Metric Filters - CAIP-2 Network Identifier Support
# ============================================================================

# Metric Filter: CAIP-2 Parsing Errors
resource "aws_cloudwatch_log_metric_filter" "caip2_parsing_errors" {
  name           = "facilitator-caip2-parsing-errors"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg=\"*CAIP-2*\" || msg=\"*caip2*\"]"

  metric_transformation {
    name      = "CAIP2ParsingErrors"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Metric Filter: Unsupported x402 Version Attempts
resource "aws_cloudwatch_log_metric_filter" "unsupported_version" {
  name           = "facilitator-unsupported-x402-version"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg=\"*Unsupported x402 version*\" || msg=\"*unsupported version*\"]"

  metric_transformation {
    name      = "UnsupportedVersionAttempts"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# ============================================================================
# Metric Filters - v2 Settlement Operations
# ============================================================================

# Metric Filter: v2 Settlement Success
resource "aws_cloudwatch_log_metric_filter" "v2_settlement_success" {
  name           = "facilitator-v2-settlement-success"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=INFO, msg=\"Settlement successful\" || msg=\"*settlement*successful*\", ..., x402_version=2]"

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
  pattern        = "[time, level=ERROR, msg=\"*settlement*failed*\" || msg=\"*settlement*error*\", ..., x402_version=2]"

  metric_transformation {
    name      = "V2SettlementFailure"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# ============================================================================
# Metric Filters - v2 Verification Operations
# ============================================================================

# Metric Filter: v2 Verification Success
resource "aws_cloudwatch_log_metric_filter" "v2_verification_success" {
  name           = "facilitator-v2-verification-success"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=INFO, msg=\"*verification*successful*\" || msg=\"Payment verification successful\", ..., x402_version=2]"

  metric_transformation {
    name      = "V2VerificationSuccess"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# Metric Filter: v2 Verification Failure
resource "aws_cloudwatch_log_metric_filter" "v2_verification_failure" {
  name           = "facilitator-v2-verification-failure"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg=\"*verification*failed*\" || msg=\"*verification*error*\", ..., x402_version=2]"

  metric_transformation {
    name      = "V2VerificationFailure"
    namespace = "Facilitator/Protocol"
    value     = "1"
    unit      = "Count"
  }
}

# ============================================================================
# CloudWatch Alarms
# ============================================================================

# Alarm: CAIP-2 Parsing Errors High
resource "aws_cloudwatch_metric_alarm" "caip2_parsing_errors_high" {
  alarm_name          = "facilitator-caip2-parsing-errors-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "CAIP2ParsingErrors"
  namespace           = "Facilitator/Protocol"
  period              = 300 # 5 minutes
  statistic           = "Sum"
  threshold           = 5 # Alert if more than 5 parsing errors in 5 minutes
  alarm_description   = "Alert when CAIP-2 network identifier parsing fails frequently"
  treat_missing_data  = "notBreaching"

  alarm_actions = [] # Add SNS topic ARN here for notifications

  tags = {
    Name        = "facilitator-caip2-parsing-alarm"
    Environment = var.environment
    Protocol    = "x402-v2"
  }
}

# Alarm: v2 Settlement Failure Rate High
resource "aws_cloudwatch_metric_alarm" "v2_settlement_failure_rate" {
  alarm_name          = "facilitator-v2-settlement-failure-rate-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "V2SettlementFailure"
  namespace           = "Facilitator/Protocol"
  period              = 300 # 5 minutes
  statistic           = "Sum"
  threshold           = 5 # Alert if more than 5 failures in 5 minutes
  alarm_description   = "Alert when v2 settlement failure rate is high"
  treat_missing_data  = "notBreaching"

  alarm_actions = [] # Add SNS topic ARN here for notifications

  tags = {
    Name        = "facilitator-v2-settlement-failure-alarm"
    Environment = var.environment
    Protocol    = "x402-v2"
  }
}

# Alarm: Unexpected v1 Traffic Drop (possible client migration issue)
resource "aws_cloudwatch_metric_alarm" "v1_traffic_unexpected_drop" {
  alarm_name          = "facilitator-x402-v1-traffic-sudden-drop"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  metric_name         = "X402V1Requests"
  namespace           = "Facilitator/Protocol"
  period              = 3600 # 1 hour
  statistic           = "Sum"
  threshold           = 5 # Alert if less than 5 v1 requests/hour (adjust based on baseline)
  alarm_description   = "Alert when v1 traffic drops unexpectedly (may indicate client issues)"
  treat_missing_data  = "notBreaching"

  # Only enable this alarm after establishing baseline traffic
  # Comment out initially, enable after 1 week of monitoring
  # alarm_actions = []

  tags = {
    Name        = "facilitator-v1-traffic-drop-alarm"
    Environment = var.environment
    Protocol    = "x402-v1"
    Enabled     = "false" # Set to true after baseline established
  }
}

# ============================================================================
# CloudWatch Dashboard - x402 v2 Migration
# ============================================================================

resource "aws_cloudwatch_dashboard" "x402_v2_migration" {
  dashboard_name = "facilitator-x402-v2-migration"

  dashboard_body = jsonencode({
    widgets = [
      # Row 1: Protocol Version Adoption
      {
        type   = "metric"
        x      = 0
        y      = 0
        width  = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "X402V1Requests", { stat = "Sum", label = "v1 Requests", color = "#FF9900" }],
            [".", "X402V2Requests", { stat = "Sum", label = "v2 Requests", color = "#1f77b4" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "x402 Protocol Version Adoption"
          yAxis = {
            left = {
              min = 0
            }
          }
          annotations = {
            horizontal = [
              {
                label = "v2 Launch"
                value = 0
                fill  = "above"
                color = "#2ca02c"
              }
            ]
          }
        }
      },
      # Row 1: Protocol Version Percentage
      {
        type   = "metric"
        x      = 12
        y      = 0
        width  = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "X402V1Requests", { stat = "Sum", id = "v1" }],
            [".", "X402V2Requests", { stat = "Sum", id = "v2" }],
            [{
              expression = "100 * v2 / (v1 + v2)"
              label      = "v2 Adoption %"
              id         = "v2_percent"
              color      = "#1f77b4"
            }]
          ]
          period = 300
          region = var.aws_region
          title  = "v2 Adoption Percentage"
          yAxis = {
            left = {
              min = 0
              max = 100
            }
          }
        }
      },
      # Row 2: CAIP-2 Parsing Errors
      {
        type   = "metric"
        x      = 0
        y      = 6
        width  = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "CAIP2ParsingErrors", { stat = "Sum", color = "#d62728" }],
            [".", "UnsupportedVersionAttempts", { stat = "Sum", color = "#ff7f0e" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "CAIP-2 Parsing & Version Errors"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      # Row 2: v2 Settlement Operations
      {
        type   = "metric"
        x      = 12
        y      = 6
        width  = 12
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
      # Row 3: v2 Verification Operations
      {
        type   = "metric"
        x      = 0
        y      = 12
        width  = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "V2VerificationSuccess", { stat = "Sum", label = "Success", color = "#2ca02c" }],
            [".", "V2VerificationFailure", { stat = "Sum", label = "Failure", color = "#d62728" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "v2 Verification Operations"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      # Row 3: v2 Success Rate
      {
        type   = "metric"
        x      = 12
        y      = 12
        width  = 12
        height = 6
        properties = {
          metrics = [
            ["Facilitator/Protocol", "V2SettlementSuccess", { stat = "Sum", id = "settle_success", visible = false }],
            [".", "V2SettlementFailure", { stat = "Sum", id = "settle_fail", visible = false }],
            [{
              expression = "100 * settle_success / (settle_success + settle_fail)"
              label      = "Settlement Success Rate %"
              id         = "settle_rate"
              color      = "#2ca02c"
            }]
          ]
          period = 300
          region = var.aws_region
          title  = "v2 Settlement Success Rate"
          yAxis = {
            left = {
              min = 0
              max = 100
            }
          }
        }
      },
      # Row 4: Recent v2 Log Events
      {
        type   = "log"
        x      = 0
        y      = 18
        width  = 24
        height = 6
        properties = {
          query  = "SOURCE '${aws_cloudwatch_log_group.facilitator.name}' | fields @timestamp, network, x402_version, msg | filter x402_version = 2 | sort @timestamp desc | limit 100"
          region = var.aws_region
          title  = "Recent v2 Log Events"
          view   = "table"
        }
      },
      # Row 5: v2 Network Distribution (CAIP-2 format)
      {
        type   = "log"
        x      = 0
        y      = 24
        width  = 12
        height = 6
        properties = {
          query  = "SOURCE '${aws_cloudwatch_log_group.facilitator.name}' | fields network | filter x402_version = 2 | stats count() as requests by network | sort requests desc"
          region = var.aws_region
          title  = "v2 Network Distribution (CAIP-2)"
          view   = "table"
        }
      },
      # Row 5: Error Summary
      {
        type   = "log"
        x      = 12
        y      = 24
        width  = 12
        height = 6
        properties = {
          query  = "SOURCE '${aws_cloudwatch_log_group.facilitator.name}' | fields @timestamp, msg | filter level = \"ERROR\" and x402_version = 2 | stats count() as errors by msg | sort errors desc | limit 10"
          region = var.aws_region
          title  = "Top v2 Error Messages"
          view   = "table"
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

output "v2_metrics_namespace" {
  description = "CloudWatch metrics namespace for v2 protocol metrics"
  value       = "Facilitator/Protocol"
}

# ============================================================================
# Notes
# ============================================================================

# Deployment Steps:
# 1. Apply this Terraform configuration: terraform apply
# 2. Deploy dual-support application (v1+v2) to ECS
# 3. Monitor dashboard at the URL output above
# 4. After 6 months, deprecate v1 support

# Cost Breakdown:
# - Metric filters: 7 filters × $0.50/month = $3.50/month
# - Dashboard: $0/month (included in free tier)
# - Alarms: 3 alarms × $0.10/month = $0.30/month
# - Total: ~$4/month

# Maintenance:
# - Review dashboard weekly during migration period
# - Adjust alarm thresholds after establishing baseline
# - Remove v1 metrics after full v2 migration (6+ months)
