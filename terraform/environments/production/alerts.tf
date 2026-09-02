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
    # SQS tag values accept only letters, digits, whitespace and _ . : / = + - @
    # -- no commas. The full explanation lives in the comment above the resource,
    # which is where it belongs anyway.
    Purpose = "sns-backup-subscriber"
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
# Whole-ALB p99 latency -- the coarse backstop under the per-rail alarms
# ============================================================================
#
# RETUNED 2026-09-01, threshold 2s -> 12s. The 2026-08-28 calibration below is
# preserved because its reasoning about PERIOD is still right and still load
# bearing; only its threshold was wrong, and it was wrong for a reason worth
# writing down.
#
# The four "healthy" days it was calibrated against (2026-08-25/28, p99 0.4s)
# were quiet because there was no WRITE traffic in them, not because the
# service was fast. Every /settle and /feedback waits for an on-chain receipt
# and lands at 5-7s; the moment settlements resumed, p99 sat at ~7s
# permanently and this alarm paged continuously through an entirely healthy
# day. A 2s threshold on a mixed read+write population is not satisfiable.
#
# The 2s intent now lives in aws_cloudwatch_metric_alarm.latency_reads_p99
# (latency-split.tf), measured against the read target group where it holds.
# The write rail has its own alarm at 15s. What is left for THIS alarm is the
# coarse whole-system net: 12s is above every healthy sample ever measured
# (max 7.7s) and below the 13.2s the 2026-09-01 nonce incident produced, and
# unlike aws_cloudwatch_metric_alarm.orphan_latency_p99 it reads at period=300
# where p99 is a percentile rather than a near-max.
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
  threshold           = 12
  treat_missing_data  = "notBreaching"

  alarm_description = "Facilitator ALB p99 latency sustained >12s for at least 25 of the last 30 minutes, across ALL routes. Coarse whole-system backstop under latency-reads-p99 (2s, read target group) and latency-writes-p99 (15s, write target group), which are the alarms that say WHICH rail broke. Retuned from 2s to 12s on 2026-09-01: the original 2s was calibrated during a window with no write traffic, and paged continuously once settlements resumed, because a healthy /settle takes 5-7s waiting for an on-chain receipt."

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

# ============================================================================
# Nonce storms on the EVM write rail
# ============================================================================
# Added 2026-09-01. The 5xx alarm (aws_cloudwatch_metric_alarm.orphan_5xx_errors,
# alerts-imported.tf) already catches the SYMPTOM and did fire correctly through
# the 2026-09-01 22:16-23:13 UTC episode. What no alarm says is WHY, and the why
# has been recurring unattended:
#
#   08-31 01:42 UTC +6h :    40
#   08-31 07:42 UTC +6h :     0
#   08-31 13:42 UTC +6h :     0
#   08-31 19:41 UTC +6h : 8,480
#   09-01 01:41 UTC +6h : 3,494
#   09-01 07:41 UTC +6h :     0
#   09-01 13:41 UTC +6h :     0
#   09-01 19:41 UTC +6h :   642
#
# 12,656 occurrences in 48h, in overnight bursts, against a baseline of exactly
# zero for 12h at a stretch. During a burst the in-memory nonce counter runs
# ahead of chain state -- measured tx=1603 against state=1555 on arbitrum with
# wallet 0x103040545AC5031A11E8C03dd11324C7333a13C7 -- and every escrow settle
# in flight fails until it converges again.
#
# ROOT CAUSE FOUND AND FIXED -- see b4170d76 on main, deployed 2026-09-02 00:29
# UTC as 2.8.0-53b2e68. resync_target ratcheted to high_water + 1, one PAST the
# nonce that had just failed, and its healing branch only trusts the chain after
# 120s with no allocations -- a window a continuous burst never leaves. Zero
# occurrences since that deploy.
#
# This alarm is therefore NOT tracking an open incident. It exists because the
# storm ran for at least 48h before anyone looked, and nothing would have said
# so: the 5xx alarm fires on the symptom and clears between bursts, and the
# nonce counter is invisible from outside. If the ratchet ever comes back, or a
# different path reintroduces one, this says so the same night.
#
# Do not read a firing here as "the writer lease broke". The lease warnings that
# accompanied the 2026-09-01 storm ("EVM writer lease holder endpoint is
# unknown; writes will 503") were CONCURRENT, not causal -- see b0f9bafe, which
# retracts exactly that attribution.
#
# Filter-pattern note: this log group is ANSI-coloured and the colour codes
# split key=value tokens in the raw bytes, so patterns like "status=500" match
# nothing. "nonce too high" sits inside the quoted RPC message where no colour
# code lands -- verified against the incident window, 256 matches in 90 min.
resource "aws_cloudwatch_log_metric_filter" "evm_nonce_desync" {
  name           = "facilitator-evm-nonce-desync"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "\"nonce too high\""

  metric_transformation {
    name      = "EvmNonceDesync"
    namespace = "Facilitator/ChainRail"
    value     = "1"
    unit      = "Count"
    # No default_value: absent data is treated as notBreaching below rather
    # than published as zero, which keeps the metric free when nothing is wrong.
  }
}

resource "aws_cloudwatch_metric_alarm" "evm_nonce_desync" {
  alarm_name          = "facilitator-${var.environment}-evm-nonce-desync"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  datapoints_to_alarm = 2
  metric_name         = aws_cloudwatch_log_metric_filter.evm_nonce_desync.metric_transformation[0].name
  namespace           = aws_cloudwatch_log_metric_filter.evm_nonce_desync.metric_transformation[0].namespace
  period              = 300
  statistic           = "Sum"
  threshold           = 5
  treat_missing_data  = "notBreaching"

  alarm_description = "EVM nonce desync: the facilitator's in-memory nonce counter has run ahead of chain state and settlements are failing with 'nonce too high'. Healthy baseline is exactly 0 (measured over multiple 6h windows); bursts reach thousands. Threshold >5 per 5 min for 2 of 3 periods pages ~10 min into a storm and ignores a stray retry. Root cause of the 2026-09-01 storm was the resync ratchet, fixed in b4170d76 and deployed 2026-09-02 00:29 UTC; zero occurrences since. A firing means a ratchet is back, not that the writer lease broke -- b0f9bafe retracts that attribution."

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-evm-nonce-desync"
    Environment = var.environment
  }
}

# ============================================================================
# DX402 evidence storage: the failure that is silent by design
# ============================================================================
#
# When Pinata refuses a write, `FallbackEvidenceStore` writes to S3 instead and
# the payment succeeds. That is the correct behaviour -- DX402 must never fail a
# payment -- and it is exactly why nobody would notice. The anchor returns 201,
# the buyer gets their bytes, and the only trace is one `warn!` line.
#
# Three different problems arrive through this one door:
#
#   - The Pinata JWT expires. It carries an `exp` and the current one runs out
#     on 2026-12-19. From that moment every anchor falls back, permanently, and
#     nothing else says so.
#   - Quota. The free tier caps files and storage; the dashboard's counter does
#     NOT include private files, so the number an operator reads there is not
#     the number that will run out.
#   - Any Pinata outage or 5xx.
#
# Until v2.3.0 this was worse than invisible: the pointer was predicted from the
# PRIMARY before the upload and the real one discarded, so a fallback write left
# a facilitator-SIGNED receipt naming an IPFS object that never existed, and the
# read path treats the resulting NotFound as a verdict. That is fixed -- the
# pointer is reconciled now -- so a fallback is an orderly degradation. This
# alarm exists so it is also a VISIBLE one.
resource "aws_cloudwatch_log_metric_filter" "dx402_store_fallback" {
  name           = "facilitator-dx402-store-fallback"
  log_group_name = aws_cloudwatch_log_group.facilitator.name

  # Substring match, not a positional pattern: the log line is emitted by
  # `tracing` with structured fields whose order is not a contract, and a
  # positional filter that silently stops matching is how an alarm becomes
  # decoration.
  pattern = "dx402_primary_store_unavailable"

  metric_transformation {
    name          = "DX402StoreFallback"
    namespace     = "Facilitator/DX402"
    value         = "1"
    unit          = "Count"
    default_value = "0"
  }
}

# One fallback is worth knowing about; it is not a blip that resolves itself.
# Whatever caused it -- expiry, quota, an outage -- is still true for the next
# anchor, so `evaluation_periods = 1` is deliberate.
resource "aws_cloudwatch_metric_alarm" "dx402_store_fallback" {
  alarm_name  = "facilitator-${var.environment}-dx402-store-fallback"
  namespace   = "Facilitator/DX402"
  metric_name = "DX402StoreFallback"
  statistic   = "Sum"

  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"

  # Absence of data is the normal state: no anchors in five minutes means no
  # fallbacks, not an unknown. Treating it as breaching would page all night.
  treat_missing_data = "notBreaching"

  alarm_description = join(" ", [
    "DX402 evidence is being written to S3 because Pinata refused.",
    "Payments are unaffected and evidence is still durable -- the pointer is",
    "reconciled since v2.3.0 -- but the primary store is down, out of quota, or",
    "the JWT expired. Check the JWT's `exp` (the current one ends 2026-12-19),",
    "then the Pinata dashboard. Note its file counter excludes PRIVATE files,",
    "which is what DX402 writes.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-dx402-store-fallback"
    Environment = var.environment
  }
}
