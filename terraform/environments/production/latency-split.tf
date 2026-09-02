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

# APPLY THIS RULE ONLY AFTER aws_lb_target_group.writes HAS HEALTHY TARGETS.
#
# Terraform has no way to express "wait until ECS finished registering". If it
# creates this rule in the same apply that adds the target group, the ALB starts
# forwarding /settle to a target group with nothing healthy in it and answers
# 503 on the money rail for as long as registration takes -- healthy_threshold 2
# at interval 60 means up to ~2 minutes of failed settlements, on top of the
# rolling ECS deployment the service change triggers.
#
# So apply in two steps:
#
#   1. terraform apply -target=aws_lb_target_group.writes \
#                      -target=aws_ecs_service.facilitator
#      aws elbv2 describe-target-health --region us-east-2 \
#        --target-group-arn <writes tg arn>     # wait for all "healthy"
#   2. terraform apply -target=aws_lb_listener_rule.writes \
#                      -target=aws_cloudwatch_metric_alarm.latency_reads_p99 \
#                      -target=aws_cloudwatch_metric_alarm.latency_writes_p99
#
# Step 1 is safe on its own: a registered-but-unrouted target group changes no
# traffic. Step 2 is the cutover and is instant once targets are healthy.
#
# Route the endpoints that submit a transaction and wait for its receipt.
#
# ALB allows at most 5 values in one path_pattern condition and these are
# exactly 5. Anything added later needs a second rule, not a 6th value.
#
# Exact-match semantics matter here: "/register" does NOT capture
# "/register/status/{jobId}", which is a fast poll and correctly stays on the
# read rail. "/feedback/*" deliberately carries /feedback/evm/prepare too --
# prepare is fast, but it belongs to the write family and keeping the family
# whole beats shaving one fast route off it.
#
# NOT routed here on purpose: /verify (RPC read, measured 160ms avg),
# /escrow/state (103ms), /discovery/register (a store write, not on-chain).
resource "aws_lb_listener_rule" "writes" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 20

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.writes.arn
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
