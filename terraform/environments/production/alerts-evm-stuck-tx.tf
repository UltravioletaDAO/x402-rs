# Alarm for the facilitator's own EVM transactions wedged in a node's mempool.
#
# Created by the pipeline: the two addresses below are in the `-target` list of
# the "Deploy observability" step in .github/workflows/ci.yaml. A merge to `main`
# creates them. Adding a third alarm here means adding its address there too, or
# the drift gate will say so.

# ============================================================================
# A signer whose next nonce stops advancing takes the whole chain down with it,
# and nothing else notices
# ============================================================================
#
# On 2026-09-03 at 21:28:00Z Polygon's base fee was in a trough of 1.072 gwei --
# it had collapsed from its usual ~250 and was climbing back. The facilitator
# priced an escrow `release` there with alloy's default estimator, whose entire
# buffer is 2x the latest base fee, so it went out capped at 32.247 gwei. Forty
# minutes later the base fee was 248 gwei and nonce 1157 could never be mined.
#
# Nonces are strictly ordered, so the mainnet signer froze behind it: 399
# correctly priced transactions stacked up, their pooled
# `gasLimit * maxFeePerGas` reached 82.80 of the wallet's 82.86 POL, and every
# new Polygon settle was refused by the node with `insufficient funds for
# gas * price + value`.
#
# It ran for six days. None of the existing alarms could see it:
#
#   * `chain_balance_low`   -- the balance never MOVED. 82.86 POL, byte for
#                              byte, for six days. It was reserved, not spent.
#   * `chain_rpc_unreachable` -- the RPC was fine and answered every call.
#   * `orphan_5xx_errors`   -- the facilitator returned clean 4xx for a payment
#                              it could not settle. Nothing was down.
#   * `evm_nonce_desync`    -- that one matches "nonce too high", which is the
#                              opposite failure: our counter ahead of the chain.
#                              Here the counter was right and the CHAIN was stuck.
#
# The one signal that was unambiguous the whole time is the one the facilitator
# now emits: `eth_getTransactionCount(latest)` frozen while `pending` climbs.
# `src/stuck_tx_monitor.rs` polls both every two minutes per EVM network and
# warns once the head has sat still for ten minutes with a backlog behind it.
#
# Filter-pattern note: a substring, not a positional key=value pattern. This log
# group is ANSI-coloured and the colour codes split key=value tokens in the raw
# bytes -- the same reason `evm_nonce_desync` and `solana_mint_fee_payer_dry`
# match on message text. `evm_signer_transactions_stuck` is the tracing message
# itself, where no colour code lands, and it is spelled without spaces so a
# colour code cannot land in the middle of it either.
resource "aws_cloudwatch_log_metric_filter" "evm_signer_transactions_stuck" {
  name           = "facilitator-evm-signer-transactions-stuck"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "evm_signer_transactions_stuck"

  metric_transformation {
    name          = "EvmSignerTransactionsStuck"
    namespace     = "Facilitator/ChainRail"
    value         = "1"
    unit          = "Count"
    default_value = "0"
  }
}

# The monitor re-emits on every poll while the condition lasts, so a stuck
# signer produces ~2-3 lines per five-minute period per affected chain, and a
# healthy fleet produces exactly zero.
#
# Two periods rather than one: the emission is already gated on ten minutes of
# a motionless head, so this is not about riding out a blip -- it is about not
# paging on a single line from a task that restarted mid-episode and re-armed
# its own clock. Total time to page is roughly fifteen minutes, against six days.
resource "aws_cloudwatch_metric_alarm" "evm_signer_transactions_stuck" {
  alarm_name          = "facilitator-${var.environment}-evm-signer-transactions-stuck"
  namespace           = aws_cloudwatch_log_metric_filter.evm_signer_transactions_stuck.metric_transformation[0].namespace
  metric_name         = aws_cloudwatch_log_metric_filter.evm_signer_transactions_stuck.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 2
  datapoints_to_alarm = 2
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"

  # A healthy fleet emits `default_value = 0` on every poll, so a missing
  # datapoint means the facilitator is not logging at all -- which
  # `orphan_no_running_tasks` already covers. Do not double-page.
  treat_missing_data = "notBreaching"

  alarm_description = join(" ", [
    "An EVM signer's next nonce has not advanced for ten minutes while",
    "transactions pile up behind it. Every settle on that chain is failing and",
    "will keep failing until the head clears -- the balance will look untouched,",
    "because it is RESERVED by the stuck queue rather than spent. The log line",
    "carries `network`, `signer` and `first_unmined_nonce`; that nonce is the",
    "one to replace. Diagnose and unstick with",
    "`python3 scripts/polygon_destrabar_cola.py --dry-run`, and read",
    "docs/handoffs/2026-09-10-polygon-cola.md before running it with --apply.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-evm-signer-transactions-stuck"
    Environment = var.environment
  }
}
