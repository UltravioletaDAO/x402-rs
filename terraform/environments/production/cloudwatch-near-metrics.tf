# ============================================================================
# CloudWatch Metrics and Alarms for NEAR Protocol Integration
# ============================================================================

# CloudWatch Log Metric Filter - NEAR Settlement Success
resource "aws_cloudwatch_log_metric_filter" "near_settlement_success" {
  name           = "facilitator-near-settlement-success"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level, msg=\"Settlement successful\", network=near* || network=NEAR*]"

  metric_transformation {
    name      = "NEARSettlementSuccess"
    namespace = "Facilitator/NEAR"
    value     = "1"
    unit      = "Count"
  }
}

# CloudWatch Log Metric Filter - NEAR Settlement Failure
resource "aws_cloudwatch_log_metric_filter" "near_settlement_failure" {
  name           = "facilitator-near-settlement-failure"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg, network=near* || network=NEAR*]"

  metric_transformation {
    name      = "NEARSettlementFailure"
    namespace = "Facilitator/NEAR"
    value     = "1"
    unit      = "Count"
  }
}

# CloudWatch Log Metric Filter - NEAR RPC Errors
resource "aws_cloudwatch_log_metric_filter" "near_rpc_error" {
  name           = "facilitator-near-rpc-error"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg=\"*RPC*\", chain=near* || chain=NEAR*]"

  metric_transformation {
    name      = "NEARRPCError"
    namespace = "Facilitator/NEAR"
    value     = "1"
    unit      = "Count"
  }
}

# CloudWatch Log Metric Filter - NEAR Verification Success
resource "aws_cloudwatch_log_metric_filter" "near_verification_success" {
  name           = "facilitator-near-verification-success"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level, msg=\"Payment verification successful\", network=near* || network=NEAR*]"

  metric_transformation {
    name      = "NEARVerificationSuccess"
    namespace = "Facilitator/NEAR"
    value     = "1"
    unit      = "Count"
  }
}

# CloudWatch Log Metric Filter - NEAR Verification Failure
resource "aws_cloudwatch_log_metric_filter" "near_verification_failure" {
  name           = "facilitator-near-verification-failure"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "[time, level=ERROR, msg=\"*verification*failed*\", network=near* || network=NEAR*]"

  metric_transformation {
    name      = "NEARVerificationFailure"
    namespace = "Facilitator/NEAR"
    value     = "1"
    unit      = "Count"
  }
}

# ============================================================================
# CloudWatch Alarms for NEAR Operations
# ============================================================================

# Alarm: High NEAR Settlement Failure Rate
resource "aws_cloudwatch_metric_alarm" "near_settlement_failure_rate" {
  alarm_name          = "facilitator-near-settlement-failure-rate-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "NEARSettlementFailure"
  namespace           = "Facilitator/NEAR"
  period              = 300  # 5 minutes
  statistic           = "Sum"
  threshold           = 5    # Alert if more than 5 failures in 5 minutes
  alarm_description   = "Alert when NEAR settlement failure rate is high"
  treat_missing_data  = "notBreaching"

  alarm_actions = []  # Add SNS topic ARN here if you want email/SMS notifications

  tags = {
    Name        = "facilitator-near-settlement-failure-alarm"
    Environment = var.environment
    Chain       = "near"
  }
}

# Alarm: NEAR RPC Connectivity Issues
resource "aws_cloudwatch_metric_alarm" "near_rpc_errors" {
  alarm_name          = "facilitator-near-rpc-errors-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "NEARRPCError"
  namespace           = "Facilitator/NEAR"
  period              = 300  # 5 minutes
  statistic           = "Sum"
  threshold           = 10   # Alert if more than 10 RPC errors in 5 minutes
  alarm_description   = "Alert when NEAR RPC error rate is high"
  treat_missing_data  = "notBreaching"

  alarm_actions = []  # Add SNS topic ARN here if you want email/SMS notifications

  tags = {
    Name        = "facilitator-near-rpc-error-alarm"
    Environment = var.environment
    Chain       = "near"
  }
}

# ============================================================================
# CloudWatch Dashboard for NEAR Metrics (Optional)
# ============================================================================

resource "aws_cloudwatch_dashboard" "near_operations" {
  dashboard_name = "facilitator-near-operations"

  dashboard_body = jsonencode({
    widgets = [
      {
        type = "metric"
        properties = {
          metrics = [
            ["Facilitator/NEAR", "NEARSettlementSuccess", { stat = "Sum", label = "Settlement Success" }],
            [".", "NEARSettlementFailure", { stat = "Sum", label = "Settlement Failure" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "NEAR Settlement Operations"
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
            ["Facilitator/NEAR", "NEARVerificationSuccess", { stat = "Sum", label = "Verification Success" }],
            [".", "NEARVerificationFailure", { stat = "Sum", label = "Verification Failure" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "NEAR Payment Verification"
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
            ["Facilitator/NEAR", "NEARRPCError", { stat = "Sum" }]
          ]
          period = 300
          stat   = "Sum"
          region = var.aws_region
          title  = "NEAR RPC Errors"
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type = "log"
        properties = {
          query   = "SOURCE '/ecs/facilitator-production' | fields @timestamp, @message | filter @message like /near/ or @message like /NEAR/ | sort @timestamp desc | limit 100"
          region  = var.aws_region
          title   = "Recent NEAR Log Events"
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

output "near_dashboard_url" {
  description = "CloudWatch Dashboard URL for NEAR operations"
  value       = "https://console.aws.amazon.com/cloudwatch/home?region=${var.aws_region}#dashboards:name=${aws_cloudwatch_dashboard.near_operations.dashboard_name}"
}
