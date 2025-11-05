# ============================================================================
# CLOUDWATCH OBSERVABILITY - Logs, Metrics, Alarms
# ============================================================================
# COST OPTIMIZATIONS:
# - 7-day log retention (vs 30+ days)
# - Container Insights enabled (essential for visibility)
# - Basic alarms only (CPU, memory, task count)

# ----------------------------------------------------------------------------
# CloudWatch Log Groups (one per agent)
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_log_group" "agents" {
  for_each = var.agents

  name              = "/ecs/${var.project_name}-${var.environment}/${each.key}"
  retention_in_days = var.log_retention_days

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-logs"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# CloudWatch Metric Filters (Optional - for custom metrics)
# ----------------------------------------------------------------------------
# Example: Count errors in logs

resource "aws_cloudwatch_log_metric_filter" "error_count" {
  for_each = var.agents

  name           = "${each.key}-error-count"
  log_group_name = aws_cloudwatch_log_group.agents[each.key].name
  pattern        = "[time, request_id, level = ERROR*, ...]"

  metric_transformation {
    name      = "${each.key}ErrorCount"
    namespace = "${var.project_name}/${var.environment}"
    value     = "1"
    default_value = "0"
  }
}

# ----------------------------------------------------------------------------
# SNS Topic for Alarms (Optional)
# ----------------------------------------------------------------------------

resource "aws_sns_topic" "alarms" {
  count = var.alarm_sns_topic_name != "" ? 1 : 0

  name = var.alarm_sns_topic_name

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-alarms"
  })
}

# ----------------------------------------------------------------------------
# CloudWatch Alarms - High CPU Utilization
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_metric_alarm" "high_cpu" {
  for_each = var.enable_high_cpu_alarm ? var.agents : {}

  alarm_name          = "${var.project_name}-${var.environment}-${each.key}-high-cpu"
  alarm_description   = "Alert when ${each.key} CPU exceeds ${var.cpu_alarm_threshold}%"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "CPUUtilization"
  namespace           = "AWS/ECS"
  period              = 300 # 5 minutes
  statistic           = "Average"
  threshold           = var.cpu_alarm_threshold
  treat_missing_data  = "notBreaching"

  dimensions = {
    ServiceName = aws_ecs_service.agents[each.key].name
    ClusterName = aws_ecs_cluster.main.name
  }

  alarm_actions = var.alarm_sns_topic_name != "" ? [aws_sns_topic.alarms[0].arn] : []

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-high-cpu-alarm"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# CloudWatch Alarms - High Memory Utilization
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_metric_alarm" "high_memory" {
  for_each = var.enable_high_memory_alarm ? var.agents : {}

  alarm_name          = "${var.project_name}-${var.environment}-${each.key}-high-memory"
  alarm_description   = "Alert when ${each.key} memory exceeds ${var.memory_alarm_threshold}%"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "MemoryUtilization"
  namespace           = "AWS/ECS"
  period              = 300 # 5 minutes
  statistic           = "Average"
  threshold           = var.memory_alarm_threshold
  treat_missing_data  = "notBreaching"

  dimensions = {
    ServiceName = aws_ecs_service.agents[each.key].name
    ClusterName = aws_ecs_cluster.main.name
  }

  alarm_actions = var.alarm_sns_topic_name != "" ? [aws_sns_topic.alarms[0].arn] : []

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-high-memory-alarm"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# CloudWatch Alarms - Low Task Count (Service Health)
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_metric_alarm" "low_task_count" {
  for_each = var.enable_task_count_alarm ? var.agents : {}

  alarm_name          = "${var.project_name}-${var.environment}-${each.key}-low-tasks"
  alarm_description   = "Alert when ${each.key} has fewer than 1 running task"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 1
  metric_name         = "RunningTaskCount"
  namespace           = "ECS/ContainerInsights"
  period              = 60
  statistic           = "Average"
  threshold           = 1
  treat_missing_data  = "breaching"

  dimensions = {
    ServiceName = aws_ecs_service.agents[each.key].name
    ClusterName = aws_ecs_cluster.main.name
  }

  alarm_actions = var.alarm_sns_topic_name != "" ? [aws_sns_topic.alarms[0].arn] : []

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-low-tasks-alarm"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# CloudWatch Alarms - ALB Target Health
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_metric_alarm" "unhealthy_targets" {
  for_each = var.agents

  alarm_name          = "${var.project_name}-${var.environment}-${each.key}-unhealthy-targets"
  alarm_description   = "Alert when ${each.key} has unhealthy targets"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "UnHealthyHostCount"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  statistic           = "Average"
  threshold           = 0
  treat_missing_data  = "notBreaching"

  dimensions = {
    TargetGroup  = aws_lb_target_group.agents[each.key].arn_suffix
    LoadBalancer = aws_lb.main.arn_suffix
  }

  alarm_actions = var.alarm_sns_topic_name != "" ? [aws_sns_topic.alarms[0].arn] : []

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-unhealthy-targets-alarm"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# CloudWatch Dashboard (Optional - for visual monitoring)
# ----------------------------------------------------------------------------
# NOTE: Temporarily disabled due to metric format issues.
# Dashboard can be created manually in AWS Console or fixed later.
# All CloudWatch alarms and metrics are still active.

/* DISABLED - NEEDS METRIC FORMAT FIX
resource "aws_cloudwatch_dashboard" "main_disabled" {
  count = 0  # Disabled
  dashboard_name = "${var.project_name}-${var.environment}"

  dashboard_body = jsonencode({
    widgets = concat(
      # CPU Utilization Widgets
      [for idx, agent_name in keys(var.agents) : {
        type = "metric"
        x    = (idx % 3) * 8
        y    = floor(idx / 3) * 6
        width = 8
        height = 6
        properties = {
          metrics = [
            ["AWS/ECS", "CPUUtilization", {
              stat = "Average"
              label = "${agent_name} CPU"
              dimensions = {
                ServiceName = "${var.project_name}-${var.environment}-${agent_name}"
                ClusterName = "${var.project_name}-${var.environment}"
              }
            }]
          ]
          period = 300
          stat = "Average"
          region = var.aws_region
          title = "${agent_name} - CPU Utilization"
          yAxis = {
            left = {
              min = 0
              max = 100
            }
          }
        }
      }],
      # Memory Utilization Widgets
      [for idx, agent_name in keys(var.agents) : {
        type = "metric"
        x    = (idx % 3) * 8
        y    = floor(idx / 3) * 6 + 18
        width = 8
        height = 6
        properties = {
          metrics = [
            ["AWS/ECS", "MemoryUtilization", {
              stat = "Average"
              label = "${agent_name} Memory"
              dimensions = {
                ServiceName = "${var.project_name}-${var.environment}-${agent_name}"
                ClusterName = "${var.project_name}-${var.environment}"
              }
            }]
          ]
          period = 300
          stat = "Average"
          region = var.aws_region
          title = "${agent_name} - Memory Utilization"
          yAxis = {
            left = {
              min = 0
              max = 100
            }
          }
        }
      }],
      # ALB Request Count
      [{
        type = "metric"
        x = 0
        y = 36
        width = 24
        height = 6
        properties = {
          metrics = [
            for agent_name in keys(var.agents) :
            ["AWS/ApplicationELB", "RequestCount", {
              stat = "Sum"
              label = agent_name
              dimensions = {
                TargetGroup = aws_lb_target_group.agents[agent_name].arn_suffix
                LoadBalancer = aws_lb.main.arn_suffix
              }
            }]
          ]
          period = 300
          stat = "Sum"
          region = var.aws_region
          title = "ALB Request Count by Agent"
        }
      }]
    )
  })
}
*/

# ----------------------------------------------------------------------------
# X-Ray Sampling Rule (for distributed tracing)
# ----------------------------------------------------------------------------

resource "aws_xray_sampling_rule" "main" {
  count = var.enable_xray_tracing ? 1 : 0

  rule_name      = "${var.project_name}-${var.environment}"
  priority       = 1000
  version        = 1
  reservoir_size = 1
  fixed_rate     = 0.05 # Sample 5% of requests
  url_path       = "*"
  host           = "*"
  http_method    = "*"
  service_type   = "*"
  service_name   = "*"
  resource_arn   = "*"

  attributes = {
    Environment = var.environment
    Project     = var.project_name
  }
}
