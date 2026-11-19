# Alarms for writes the facilitator refuses before the chain: a gas estimate above the
# ceiling for its kind of call (src/chain/gas_ceiling.rs, src/erc8004/max_gas.rs) and an
# ERC-8004 write past its network's daily limit (src/erc8004/daily_cap.rs).
#
# Created by the pipeline: the four addresses below are in the `-target` list of the
# "Deploy observability" step in .github/workflows/ci.yaml.
#
# Each refusal answers the caller with a clean 4xx and sends nothing, so no 5xx, balance or
# RPC alarm sees it. A refusal is either a caller doing what the limit exists to stop, or a
# limit set too low for real traffic; both are worth a look, and neither shows anywhere else.
#
# Filter-pattern note: phrases from the tracing messages themselves, as in
# alerts-evm-stuck-tx.tf. This log group is ANSI-coloured and the colour codes split
# key=value tokens, but they never land inside a message.

resource "aws_cloudwatch_log_metric_filter" "gas_ceiling_refusals" {
  name           = "facilitator-gas-ceiling-refusals"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  # src/chain/evm.rs: the payment paths ("...for this call; not sent...") and the
  # ERC-8004 writes ("...above its gas ceiling; call not sent...").
  pattern = "?\"Gas estimate above the ceiling for this call\" ?\"ERC-8004 write above its gas ceiling\""

  metric_transformation {
    name          = "GasCeilingRefusals"
    namespace     = "Facilitator/ChainRail"
    value         = "1"
    unit          = "Count"
    default_value = "0"
  }
}

resource "aws_cloudwatch_metric_alarm" "gas_ceiling_refusals" {
  alarm_name          = "facilitator-${var.environment}-gas-ceiling-refusals"
  namespace           = aws_cloudwatch_log_metric_filter.gas_ceiling_refusals.metric_transformation[0].namespace
  metric_name         = aws_cloudwatch_log_metric_filter.gas_ceiling_refusals.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  alarm_description = join(" ", [
    "A transaction the facilitator would pay for was refused because its gas estimate is",
    "above the ceiling for its kind of call (400 gas_over_limit or erc8004_gas_over_limit).",
    "Nothing was sent. The log line carries network, kind, estimated and ceiling: a caller",
    "probing for an expensive call, or a ceiling too low for a real one (GET /config,",
    "evmGasCeilings; override EVM_MAX_GAS_<CALL>_<NETWORK> or ERC8004_MAX_GAS_<NETWORK>).",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-gas-ceiling-refusals"
    Environment = var.environment
  }
}

resource "aws_cloudwatch_log_metric_filter" "erc8004_daily_limit_refusals" {
  name           = "facilitator-erc8004-daily-limit-refusals"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "\"ERC-8004 daily write limit reached\""

  metric_transformation {
    name          = "Erc8004DailyLimitRefusals"
    namespace     = "Facilitator/ChainRail"
    value         = "1"
    unit          = "Count"
    default_value = "0"
  }
}

resource "aws_cloudwatch_metric_alarm" "erc8004_daily_limit_refusals" {
  alarm_name          = "facilitator-${var.environment}-erc8004-daily-limit-refusals"
  namespace           = aws_cloudwatch_log_metric_filter.erc8004_daily_limit_refusals.metric_transformation[0].namespace
  metric_name         = aws_cloudwatch_log_metric_filter.erc8004_daily_limit_refusals.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  alarm_description = join(" ", [
    "An ERC-8004 write was refused with 429 erc8004_daily_write_limit: a network reached its",
    "writes for the UTC day. The log line carries network and limit. Real traffic peaked near",
    "50 a day in November 2026 against a limit of 100: a burst is either abuse of the",
    "facilitator's gas or a partner's launch. Raise ERC8004_DAILY_WRITE_CAP_<NETWORK> only",
    "for the second.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-erc8004-daily-limit-refusals"
    Environment = var.environment
  }
}
