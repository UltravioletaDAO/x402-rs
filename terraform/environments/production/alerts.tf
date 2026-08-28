# ============================================================================
# Alerting: the facilitator's own SNS topic, and per-chain health alarms
# ============================================================================
#
# Until 2026-08-20 the facilitator had eight alarms and NO voice of its own:
# five had `alarm_actions = []` and the other three published to Execution
# Market's topic. We learned about our own outages from another team's email.
#
# Worse, nothing watched the chains themselves. Two production chains broke that
# week by different causes and neither announced itself:
#   - Celo ran out of gas (0.0284 CELO against 0.1134 per escrow settle),
#     409 of 973 escrow settlements failed in 24h.
#   - The Sui RPC stopped serving JSON-RPC entirely; Sui mainnet could not
#     settle at all, and we do not know for how long.
#
# Both were visible the whole time in the balances Lambda, which queries every
# chain and publishes the numbers to the landing page. Measured, displayed, and
# watched by nobody. These alarms read that same Lambda's output as metrics.

resource "aws_sns_topic" "alerts" {
  name = "facilitator-${var.environment}-alerts"

  tags = {
    Name        = "facilitator-${var.environment}-alerts"
    Environment = var.environment
  }
}

# Email confirmation is manual and one-time: AWS sends a subscribe link that a
# human must click. Until then the subscription sits in "PendingConfirmation"
# and delivers nothing -- check with:
#   aws sns list-subscriptions-by-topic --topic-arn <arn>
resource "aws_sns_topic_subscription" "alerts_email" {
  count = var.alerts_email == "" ? 0 : 1

  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = var.alerts_email
}

# ============================================================================
# SQS backup subscription -- the channel that cannot silently expire
# ============================================================================
#
# 2026-08-28: the email subscription above went unconfirmed for 3 days and AWS
# deleted it. Terraform's state still recorded it as PendingConfirmation --
# the topic had 36 alarms wired to it and zero confirmed subscribers, and
# nothing said so. A watchdog alarm cannot fix this on its own: it would have
# to publish through the same topic it is watching, or through someone else's
# (Execution Market's), which just relocates the single point of failure.
#
# `sqs` (like `lambda`) is a protocol SNS subscribes WITHOUT a pending step --
# there is no confirmation link, so there is nothing that can go unclicked and
# nothing that expires. That is the actual fix for "the only channel needs a
# human to act within 3 days," not a monitor bolted on top of it.
#
# This queue is a MAILBOX, not a consumer -- nothing drains it, and that is
# deliberate. Its only job is to be alive so `NumberOfMessagesSent` on it (or
# just its existence as a confirmed, non-expiring subscription) is proof the
# topic still has somewhere durable to deliver, and so a human who lands here
# after an email lapse can read what was missed instead of it being gone.
# message_retention_seconds is the SQS maximum (14 days) on purpose -- this
# queue's entire value is holding messages through exactly the kind of gap
# that just bit the email subscription.
resource "aws_sqs_queue" "alerts_backup" {
  name                      = "facilitator-${var.environment}-alerts-backup"
  message_retention_seconds = 1209600 # 14 days -- SQS max

  tags = {
    Name        = "facilitator-${var.environment}-alerts-backup"
    Environment = var.environment
    Purpose     = "SNS backup subscriber -- auto-confirmed, never expires, nobody drains it"
  }
}

# SNS needs an explicit grant on the QUEUE's own resource policy to deliver to
# it -- unlike email, subscribing does not implicitly grant delivery.
resource "aws_sqs_queue_policy" "alerts_backup" {
  queue_url = aws_sqs_queue.alerts_backup.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "AllowSnsSend"
        Effect    = "Allow"
        Principal = { Service = "sns.amazonaws.com" }
        Action    = "sqs:SendMessage"
        Resource  = aws_sqs_queue.alerts_backup.arn
        Condition = {
          ArnEquals = { "aws:SourceArn" = aws_sns_topic.alerts.arn }
        }
      }
    ]
  })
}

resource "aws_sns_topic_subscription" "alerts_backup_sqs" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "sqs"
  endpoint  = aws_sqs_queue.alerts_backup.arn
}

# ============================================================================
# Per-chain health, derived from the balances Lambda
# ============================================================================
#
# The Lambda already queries every chain concurrently and returns null for any
# it could not read. That null IS the health signal -- an unreachable RPC, a
# deprecated endpoint, a dead node all produce it. It now emits that as
# ChainRpcHealthy (1/0) plus ChainNativeBalance, per chain.
#
# Deliberately NOT alarming on a single failed read: public RPCs blip. The
# alarms below require the condition to persist across evaluation periods, so a
# transient failure stays quiet and a real outage does not.

locals {
  # Chains whose health we alarm on. Mainnet only -- a broken testnet RPC is not
  # worth waking anyone, and testnet faucets run dry as a matter of course.
  #
  # min_native is a FLOOR, not a target: roughly the cost of ~100 escrow
  # settlements on that chain at the gas prices measured 2026-08-20. Celo's is
  # the one that matters most -- it is the chain that actually ran dry.
  monitored_chains = {
    "celo-mainnet"      = { min_native = 12.0 }   # ~0.1134/settle at 202 gwei
    "ethereum-mainnet"  = { min_native = 0.0035 } # L1; refill well before this
    "arbitrum-mainnet"  = { min_native = 0.0025 } # L1 data fee not in gasPrice
    "polygon-mainnet"   = { min_native = 20.0 }
    "base-mainnet"      = { min_native = 0.005 }
    "optimism-mainnet"  = { min_native = 0.005 }
    "avalanche-mainnet" = { min_native = 0.2 }
    "monad-mainnet"     = { min_native = 6.0 }
    "sui-mainnet"       = { min_native = 1.0 }
    "solana-mainnet"    = { min_native = 0.02 }
    "stellar-mainnet"   = { min_native = 5.0 } # lowest of the non-EVM family
    "near-mainnet"      = { min_native = 1.0 }
    "algorand-mainnet"  = { min_native = 5.0 }
    "xrpl-mainnet"      = { min_native = 5.0 }
  }
}

# A chain we cannot read at all. This is the alarm that would have caught Sui.
resource "aws_cloudwatch_metric_alarm" "chain_rpc_unreachable" {
  for_each = local.monitored_chains

  alarm_name          = "facilitator-chain-unreachable-${each.key}"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 3 # ~45 min at a 15-min schedule: past any RPC blip
  datapoints_to_alarm = 3
  metric_name         = "ChainRpcHealthy"
  namespace           = "Facilitator/Chains"
  period              = 900
  statistic           = "Maximum"
  threshold           = 1
  alarm_description   = "Cannot read ${each.key} at all -- dead RPC, deprecated endpoint or unreachable node. The facilitator cannot settle on this chain."

  # Missing data is NOT "fine" here: if the Lambda stops running we lose the
  # only per-chain signal we have, and that silence is exactly the failure mode
  # this alarm exists to end.
  treat_missing_data = "breaching"

  dimensions = { Chain = each.key }

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-chain-unreachable-${each.key}"
    Environment = var.environment
    Chain       = each.key
  }
}

# A chain that still answers but cannot pay for gas. This is the Celo alarm.
resource "aws_cloudwatch_metric_alarm" "chain_balance_low" {
  for_each = local.monitored_chains

  alarm_name          = "facilitator-chain-balance-low-${each.key}"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  datapoints_to_alarm = 2
  metric_name         = "ChainNativeBalance"
  namespace           = "Facilitator/Chains"
  period              = 900
  statistic           = "Minimum"
  threshold           = each.value.min_native
  alarm_description   = "Facilitator wallet on ${each.key} is below ${each.value.min_native} native -- roughly 100 settles left. Refill before it reaches zero and settlements start failing."

  # Here missing data is genuinely ambiguous (we could not read the chain), and
  # chain_rpc_unreachable already covers that case. Do not double-page.
  treat_missing_data = "missing"

  dimensions = { Chain = each.key }

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-chain-balance-low-${each.key}"
    Environment = var.environment
    Chain       = each.key
  }
}

# ============================================================================
# Schedule: the Lambda has to actually run
# ============================================================================
#
# It was only ever invoked when someone opened the landing page, so with no
# visitors there were no datapoints -- and an alarm with no datapoints tells you
# nothing. Every 15 minutes gives each alarm real data to evaluate.

resource "aws_cloudwatch_event_rule" "balances_schedule" {
  name                = "facilitator-${var.environment}-balances-schedule"
  description         = "Run the balances Lambda so per-chain health/balance metrics exist even with no landing-page traffic"
  schedule_expression = "rate(15 minutes)"

  tags = {
    Name        = "facilitator-${var.environment}-balances-schedule"
    Environment = var.environment
  }
}

resource "aws_cloudwatch_event_target" "balances_schedule" {
  rule      = aws_cloudwatch_event_rule.balances_schedule.name
  target_id = "balances-lambda"
  arn       = aws_lambda_function.balances.arn
}

resource "aws_lambda_permission" "balances_schedule" {
  statement_id  = "AllowExecutionFromEventBridge"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.balances.function_name
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.balances_schedule.arn
}

# ============================================================================
# Early-warning p99 latency -- the gap the imported >10s alarm leaves open
# ============================================================================
#
# aws_cloudwatch_metric_alarm.orphan_latency_p99 (alerts-imported.tf) requires
# >10s for 3 of 5 ONE-MINUTE periods. At this traffic volume that period is
# too small to trust: pulling 4 days of real p99 at period=60 shows adjacent
# minutes swinging from 0.05s to 29s while the alarm sat OK the whole time --
# p99 of a ~15-20 request sample is closer to "max" than a percentile.
#
# At period=300 (5 min, ~75-100 samples) the same 4 quiet days show p99
# mostly under 0.5s with a handful of ISOLATED single-period spikes to
# 2-4s -- never two in a row. But the 12 days before that (2026-08-19 through
# 08-24, the diagnosed degradation window) show p99 sustained at 2-8s across
# DOZENS of consecutive 30-min-aggregated periods, for hours at a stretch, on
# at least four separate days -- exactly what the >10s alarm missed, per the
# handoff's "no vio dos horas seguidas entre 5 y 8s."
#
# So: threshold 2s as asked, but requiring 5 of the last 6 five-minute periods
# (~25-30 min sustained) rather than a single datapoint. That filters every
# isolated blip measured in the quiet window while still catching the real
# degradation episodes within their first half hour -- markedly earlier than
# waiting for a >10s spike to land three separate times.
#
# Verified against real CloudWatch data 2026-08-28. Re-verify before relying
# on this if much time has passed -- traffic patterns shift.
resource "aws_cloudwatch_metric_alarm" "latency_p99_early" {
  alarm_name          = "facilitator-${var.environment}-latency-p99-early"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 6
  datapoints_to_alarm = 5
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 300
  extended_statistic  = "p99"
  threshold           = 2
  treat_missing_data  = "notBreaching"

  alarm_description = "Facilitator ALB p99 latency sustained >2s for at least 25 of the last 30 minutes -- early warning ahead of aws_cloudwatch_metric_alarm.orphan_latency_p99's >10s/3-of-5min threshold. Calibrated 2026-08-28 against 4 days of healthy p99 (isolated single-period spikes only, never sustained) and the 2026-08-19/24 degradation window (sustained 2-8s for hours)."

  dimensions = {
    LoadBalancer = aws_lb.main.arn_suffix
  }

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-latency-p99-early"
    Environment = var.environment
  }
}
