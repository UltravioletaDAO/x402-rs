# ============================================================================
# The three orphaned alarms -- created by hand, never in Terraform
# ============================================================================
#
# facilitator-production-5xx-errors, facilitator-production-latency-p99 and
# facilitator-production-no-running-tasks predate this repo's Terraform-managed
# alerting (alerts.tf, 2026-08-20) and were created directly in the console.
# They publish to Execution Market's topic (em-production-mcp-alerts), which
# is deliberate -- EM's escrow/reputation ops route through this facilitator,
# so their team already gets paged on our outages. Saul's call
# (docs/handoffs/2026-08-22-continuar-desde-wsl-alertas-y-performance.md §7.2):
# keep EM's topic on these AND add our own (aws_sns_topic.alerts, alerts.tf),
# not replace it.
#
# The resource blocks below describe the alarms EXACTLY as they exist in AWS
# today (verified via `aws cloudwatch describe-alarms`, 2026-08-28) except for
# one deliberate addition: aws_sns_topic.alerts.arn is appended to the action
# lists that already have EM's topic (never added to an empty array -- adding
# ok_actions where the live alarm has none would be a behavior change beyond
# what was decided). That means `terraform plan` right after import will NOT
# be a no-op -- it will show exactly one change per alarm, adding our topic.
# That diff IS the intended change, not import drift; review it, but do not
# be alarmed that it isn't empty.
#
# ----------------------------------------------------------------------------
# Import (read-only until you actually run these -- see the version trap at
# the top of the handoff: local Terraform MUST be 1.9.8, or importing rewrites
# the remote state to a format CI's 1.9.8 can no longer read):
#
#   terraform import aws_cloudwatch_metric_alarm.orphan_5xx_errors      facilitator-production-5xx-errors
#   terraform import aws_cloudwatch_metric_alarm.orphan_latency_p99     facilitator-production-latency-p99
#   terraform import aws_cloudwatch_metric_alarm.orphan_no_running_tasks facilitator-production-no-running-tasks
#
# Then `terraform plan` and read it before applying anything.
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_metric_alarm" "orphan_5xx_errors" {
  alarm_name          = "facilitator-production-5xx-errors"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  datapoints_to_alarm = 2
  metric_name         = "HTTPCode_Target_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 300
  statistic           = "Sum"
  threshold           = 2
  treat_missing_data  = "notBreaching"

  alarm_description = "Facilitator target returned HTTP 5xx -- settle/register/feedback rail failing (money rail, every EM escrow op routes through it). Retuned 2026-08-20 from >0 (21 emails/14d, 12 spurious) to >2 sustained."

  dimensions = {
    LoadBalancer = aws_lb.main.arn_suffix
    TargetGroup  = aws_lb_target_group.main.arn_suffix
  }

  alarm_actions = [
    "arn:aws:sns:us-east-2:518898403364:em-production-mcp-alerts",
    aws_sns_topic.alerts.arn,
  ]
  ok_actions = []

  tags = {
    Name        = "facilitator-production-5xx-errors"
    Environment = var.environment
    Imported    = "true"
  }
}

resource "aws_cloudwatch_metric_alarm" "orphan_latency_p99" {
  alarm_name          = "facilitator-production-latency-p99"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 5
  datapoints_to_alarm = 3
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  extended_statistic  = "p99"
  threshold           = 10
  treat_missing_data  = "notBreaching"

  alarm_description = "INC-2026-07-06: Facilitator ALB p99 latency > 10s for 3 of 5 minutes -- healthy /settle baseline is 5-7s (7d hourly p99 max 5.4s), 10s threshold avoids alarming the healthy state. See aws_cloudwatch_metric_alarm.latency_p99_early (alerts.tf) for the earlier-warning companion added 2026-08-28 -- this one alone missed multi-hour 5-8s degradation episodes on 2026-08-19 through 08-24."

  dimensions = {
    LoadBalancer = aws_lb.main.arn_suffix
  }

  alarm_actions = [
    "arn:aws:sns:us-east-2:518898403364:em-production-mcp-alerts",
    aws_sns_topic.alerts.arn,
  ]
  ok_actions = []

  tags = {
    Name        = "facilitator-production-latency-p99"
    Environment = var.environment
    Imported    = "true"
  }
}

resource "aws_cloudwatch_metric_alarm" "orphan_no_running_tasks" {
  alarm_name          = "facilitator-production-no-running-tasks"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  datapoints_to_alarm = 2
  metric_name         = "RunningTaskCount"
  namespace           = "ECS/ContainerInsights"
  period              = 300
  statistic           = "Minimum"
  threshold           = 1
  treat_missing_data  = "breaching"

  alarm_description = "Facilitator has zero running ECS tasks -- service is DOWN, all EM escrow/reputation ops fail"

  dimensions = {
    ServiceName = aws_ecs_service.facilitator.name
    ClusterName = aws_ecs_cluster.main.name
  }

  alarm_actions = [
    "arn:aws:sns:us-east-2:518898403364:em-production-mcp-alerts",
    aws_sns_topic.alerts.arn,
  ]
  ok_actions = [
    "arn:aws:sns:us-east-2:518898403364:em-production-mcp-alerts",
    aws_sns_topic.alerts.arn,
  ]

  tags = {
    Name        = "facilitator-production-no-running-tasks"
    Environment = var.environment
    Imported    = "true"
  }
}
