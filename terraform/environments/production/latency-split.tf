# ============================================================================
# Latency observability: split the write rail off the read rail
# ============================================================================
# Created 2026-09-01 after facilitator-production-latency-p99-early paged
# repeatedly through a healthy day.
#
# WHY: TargetResponseTime over the whole ALB mixes two populations with
# completely different healthy baselines, so no single threshold fits either.
# Measured over 3h of ordinary traffic (2026-09-01 22:30-01:30 UTC):
#
#   POST /settle (escrow)        166 req   avg 5.9s   max 30.2s
#   POST /feedback/evm/submit     84 req   avg 6.5s   max  7.5s
#   POST /feedback                83 req   avg 4.9s   max 28.2s
#   POST /dx402/anchor            32 req   avg 1.3s   max  2.1s
#   everything else            ~5000 req   avg <120ms max  1.0s
#
# Those seconds are not slowness, they are waiting for an on-chain receipt.
# The write rail can never come in under 2s and the read rail should never go
# over it. p99 across both is ~7s permanently, so the 2s threshold was never
# satisfiable; p90/p95 do not separate them either (measured p90 reached 4.3s
# in a write-heavy 30-min bin while p50 stayed at 47ms).
#
# The ALB emits TargetResponseTime per TargetGroup, so routing the write
# endpoints to their own target group -- same tasks, same container, same
# port -- gives each rail its own metric and its own honest threshold. This is
# a measurement change only: identical targets behind both groups.
#
# The 2026-08-28 calibration in alerts.tf was not wrong about its data, it was
# taken during 2026-08-25/28, a window with no write traffic at all (p99 0.4s).
# That is why the threshold looked reasonable and then paged continuously as
# soon as settlements resumed.

# Target group for the on-chain write endpoints. Config is deliberately
# identical to aws_lb_target_group.main -- it exists to separate the METRIC,
# not the behaviour.
resource "aws_lb_target_group" "writes" {
  name        = "facilitator-${var.environment}-writes"
  port        = 8080
  protocol    = "HTTP"
  vpc_id      = aws_vpc.main.id
  target_type = "ip"

  health_check {
    enabled             = true
    healthy_threshold   = 2
    unhealthy_threshold = 3
    timeout             = 30
    interval            = 60
    path                = "/health"
    matcher             = "200"
  }

  deregistration_delay = 30

  tags = {
    Name = "facilitator-${var.environment}-writes-tg"
  }
}

# ORDERING: THE RULE COMES FIRST, AND AWS DOES NOT ALLOW IT ANY OTHER WAY.
#
# The obvious order -- create the target group, register the service, then cut
# traffic over -- is REJECTED by ECS:
#
#   InvalidParameterException: The target group with targetGroupArn
#   .../facilitator-production-writes/... does not have an associated load
#   balancer.
#
# A target group no listener forwards to is not "associated", and ECS refuses to
# register a service against it. So the rule must exist BEFORE the service can
# join the group. Which collides head-on with the thing that must not happen: a
# rule forwarding /settle to a group with no healthy targets is a 503 on the
# money rail.
#
# A WEIGHTED forward resolves both. The rule below attaches `writes` to the load
# balancer while sending it ZERO traffic -- every write path still goes to
# `main`, byte for byte what happens today. That satisfies ECS, changes nothing
# for callers, and leaves the cutover as a separate, reversible decision.
#
#   1. terraform apply -target=aws_lb_listener_rule.writes
#      Attaches the group. No traffic moves: writes weight = 0.
#   2. terraform apply -target=aws_ecs_service.facilitator
#      ECS registers the tasks into `writes` and health checks start.
#      aws elbv2 describe-target-health --region us-east-2 \
#        --target-group-arn <writes tg arn>       # wait for all "healthy"
#   3. Flip the weights below (writes 100, main 0) and apply the rule again.
#      THIS is the cutover, and it is instant because the targets are already
#      healthy. Reverting is the same edit backwards.
#
# Only step 3 changes what any caller experiences, and only then does the
# per-rail TargetResponseTime metric start carrying the write traffic.
#
resource "aws_lb_listener_rule" "writes" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 20

  # Weights are the cutover switch. Today: everything still goes to `main`.
  # Step 3 above flips these to writes = 100, main = 0.
  action {
    type = "forward"

    forward {
      target_group {
        arn    = aws_lb_target_group.main.arn
        weight = 100
      }

      target_group {
        arn    = aws_lb_target_group.writes.arn
        weight = 0
      }

      stickiness {
        enabled  = false
        duration = 1
      }
    }
  }

  condition {
    path_pattern {
      values = [
        "/settle",
        "/feedback",
        "/feedback/*",
        "/register",
        "/dx402/anchor",
      ]
    }
  }

  tags = {
    Name = "facilitator-${var.environment}-writes-rule"
  }
}

# ----------------------------------------------------------------------------
# Read rail: this is the alarm that actually means "the facilitator is slow"
# ----------------------------------------------------------------------------
# aws_lb_target_group.main keeps every route the rule above does not claim, so
# its p99 is now a clean read-path signal. Measured healthy read p99 sits at
# 0.10-0.59s with isolated spikes to 1.0s, so 2s is ~3x headroom over the worst
# healthy sample -- the threshold the original early alarm was reaching for,
# finally measured against a population where it is satisfiable.
resource "aws_cloudwatch_metric_alarm" "latency_reads_p99" {
  alarm_name          = "facilitator-${var.environment}-latency-reads-p99"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 6
  datapoints_to_alarm = 5
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 300
  extended_statistic  = "p99"
  threshold           = 2
  treat_missing_data  = "notBreaching"

  alarm_description = "Facilitator READ path p99 > 2s sustained for 25 of the last 30 minutes. Excludes /settle, /feedback*, /register and /dx402/anchor, which are routed to aws_lb_target_group.writes because they wait for an on-chain receipt. Healthy read p99 measured 2026-09-01 at 0.10-0.59s. If this fires the service itself is slow -- it is not chain latency."

  dimensions = {
    LoadBalancer = aws_lb.main.arn_suffix
    TargetGroup  = aws_lb_target_group.main.arn_suffix
  }

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-latency-reads-p99"
    Environment = var.environment
  }
}

# ----------------------------------------------------------------------------
# Write rail: catches the chain rail degrading, without firing at healthy 7s
# ----------------------------------------------------------------------------
# Healthy write p99 measured at 7.0-7.7s across 6h, with the whole-ALB p99
# touching 13.2s during the 22:33-23:00 nonce incident. 15s sits above every
# healthy sample and below the observed incident, and 5-of-6 five-minute
# periods keeps a single slow block from paging anyone.
resource "aws_cloudwatch_metric_alarm" "latency_writes_p99" {
  alarm_name          = "facilitator-${var.environment}-latency-writes-p99"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 6
  datapoints_to_alarm = 5
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 300
  extended_statistic  = "p99"
  threshold           = 15
  treat_missing_data  = "notBreaching"

  alarm_description = "Facilitator WRITE path (/settle, /feedback*, /register, /dx402/anchor) p99 > 15s sustained for 25 of the last 30 minutes. Healthy baseline measured 2026-09-01 at 7.0-7.7s -- this is a receipt wait, not slowness. Firing means the chain rail is degrading: RPC latency, nonce contention, or a mempool that is not clearing."

  dimensions = {
    LoadBalancer = aws_lb.main.arn_suffix
    TargetGroup  = aws_lb_target_group.writes.arn_suffix
  }

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-latency-writes-p99"
    Environment = var.environment
  }
}
