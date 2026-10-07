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

  alarm_actions = [aws_sns_topic.alerts.arn]

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

  alarm_actions = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-v2-settlement-failure-alarm"
    Environment = var.environment
    Protocol    = "x402-v2"
  }
}

# facilitator-x402-v1-traffic-sudden-drop lived here: never wired to any action, its
# 5 req/h threshold never calibrated. Removed in COSTO-X402 (lote A); the operator
# deletes the live alarm with the apply.

# The facilitator-x402-v2-migration dashboard lived here. Removed in COSTO-X402
# (lote A): the v2 migration is over and the dashboard billed every month.

# ============================================================================
# Outputs
# ============================================================================


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
