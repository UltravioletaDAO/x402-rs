# Alarms for the ERC-8004 Solana identity-mint rail.
#
# Created by the pipeline: the three addresses below are in the `-target` list of
# the "Apply alerting" deploy step in .github/workflows/ci.yaml, next to every
# other alarm this stack owns. A merge to `main` creates them. Adding a fourth
# alarm here means adding its address there too, or the drift gate will say so.

# ============================================================================
# ERC-8004 Solana identity mint: the fee payer runs dry three orders of
# magnitude sooner than the settle rail does
# ============================================================================
#
# The existing `chain_balance_low` floor for solana-mainnet is 0.02 SOL, sized
# on settlements -- a settle costs ~0.000005 SOL, so 0.02 is four thousand of
# them. Minting an ERC-8004 identity costs about 0.0134 SOL, almost all of it
# rent for the three accounts it creates (agent PDA 0.0061, ATOM stats 0.0048,
# Core asset ~0.0025). At that price 0.02 SOL is ONE mint, and the settle floor
# fires long after the mint rail has already stopped working.
#
# That is what happened on 2026-09-09: ten of KarmaKadabra's twenty mints failed
# with `-32002 Transaction simulation failed: Error processing Instruction 0`,
# an error that names neither the balance nor the wallet, with the fee payer
# holding 0.000866 SOL. Nothing paged, because 0.000866 is fine for settling.
#
# Two alarms, deliberately different in kind:
#
#   mint_headroom_low  -- balance-based, fires BEFORE anything breaks.
#   mint_fee_payer_dry -- log-based, fires when a mint has actually been refused.
#
# Both are code here and nothing else: applying them is a separate, deliberate
# act. `terraform plan -target=...` for each is in
# docs/handoffs/2026-09-10-mint-atomico.md.

locals {
  # Measured by `estimate_mint_cost` and pinned by
  # `the_measured_mint_cost_is_in_the_range_karmakadabra_saw`. If that test is
  # ever updated, this number moves with it.
  solana_mint_cost_sol = 0.0134

  # Mints of margin the operator wants before being told. Matches
  # DEFAULT_MINT_HEADROOM in `src/erc8004/solana_mint.rs`, which is what the
  # facilitator's own `solana_mint_fee_payer_low` warning uses.
  solana_mint_headroom = 50
}

# The early one. Reads the same ChainNativeBalance the balances Lambda already
# publishes every 15 minutes, so it needs no new emission -- only a threshold
# that means something for this rail.
resource "aws_cloudwatch_metric_alarm" "solana_mint_headroom_low" {
  alarm_name          = "facilitator-${var.environment}-solana-mint-headroom-low"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  datapoints_to_alarm = 2
  metric_name         = "ChainNativeBalance"
  namespace           = "Facilitator/Chains"
  period              = 900
  statistic           = "Minimum"
  threshold           = local.solana_mint_cost_sol * local.solana_mint_headroom

  # Ambiguous here for the same reason as `chain_balance_low`: a missing
  # datapoint means the Lambda could not read Solana, and `chain_rpc_unreachable`
  # already covers that. Do not double-page.
  treat_missing_data = "missing"

  dimensions = { Chain = "solana-mainnet" }

  alarm_description = join(" ", [
    "The facilitator's Solana fee payer is under ${local.solana_mint_headroom}",
    "ERC-8004 identity mints of margin (${local.solana_mint_cost_sol} SOL each,",
    "almost all of it account rent). Minting still works; it stops working long",
    "before the 0.02 SOL settle floor fires, which is why this alarm exists",
    "separately. Fund F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-solana-mint-headroom-low"
    Environment = var.environment
    Chain       = "solana-mainnet"
  }
}

# The late one. Since v2.17.0 an underfunded mint is refused before it is sent,
# with a named error instead of an RPC simulation failure, and it says so in one
# structured line. This turns that line into a metric.
#
# Filter-pattern note: a substring, not a positional key=value pattern. This log
# group is ANSI-coloured and the colour codes split key=value tokens in the raw
# bytes -- the same reason `evm_nonce_desync` matches on quoted message text.
# `solana_mint_fee_payer_insufficient` is the tracing message itself, where no
# colour code lands.
resource "aws_cloudwatch_log_metric_filter" "solana_mint_fee_payer_dry" {
  name           = "facilitator-solana-mint-fee-payer-dry"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "solana_mint_fee_payer_insufficient"

  metric_transformation {
    name          = "SolanaMintFeePayerDry"
    namespace     = "Facilitator/ChainRail"
    value         = "1"
    unit          = "Count"
    default_value = "0"
  }
}

# One refusal is already a mint that did not happen for somebody. There is no
# blip to ride out: the balance that caused it is still the balance.
resource "aws_cloudwatch_metric_alarm" "solana_mint_fee_payer_dry" {
  alarm_name          = "facilitator-${var.environment}-solana-mint-fee-payer-dry"
  namespace           = aws_cloudwatch_log_metric_filter.solana_mint_fee_payer_dry.metric_transformation[0].namespace
  metric_name         = aws_cloudwatch_log_metric_filter.solana_mint_fee_payer_dry.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"

  # No mints in five minutes means no refusals, not an unknown.
  treat_missing_data = "notBreaching"

  alarm_description = join(" ", [
    "An ERC-8004 identity mint on Solana was REFUSED because the fee payer",
    "cannot cover it. Nothing was written to the chain, so there is no partial",
    "identity to clean up -- but every mint is failing until the wallet is",
    "funded. The log line carries availableLamports and requiredLamports.",
    "Fund F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq.",
  ])

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-solana-mint-fee-payer-dry"
    Environment = var.environment
  }
}
