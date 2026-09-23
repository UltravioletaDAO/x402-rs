# Alarm for an EVM RPC that answers for a different chain than the one the
# facilitator signs for, or an Arc RPC served without having answered.
#
# Created by the pipeline: the two addresses below are in the `-target` list of
# the "Deploy observability" step in .github/workflows/ci.yaml.

# ============================================================================
# The chain id we declare and the chain id the RPC answers
# ============================================================================
#
# Through 2.39.0 two testnets were served under a declared chain id that was
# not their RPC's: celo-sepolia said 44787 while its RPC answered 11142220, and
# hyperevm-testnet said 333 while its RPC answered 998. Nothing compared the
# two except for Arc.
#
# `src/chain_identity.rs` now asks every configured EVM RPC for `eth_chainId`
# once per task start and logs `evm_rpc_chain_id_mismatch` when it differs. For
# every network but Arc it is an alert and never a refusal: the network stays in
# /supported, because a refusal would have switched both testnets off over a
# number in our own table. A wrong RPC does not settle on the wrong chain -- the
# token's domain is checked on-chain and the payment fails -- so the page is for
# a human to fix the RPC or the table.
#
# Arc is admitted before it is served: a mismatch leaves Arc out of /supported
# (same `evm_rpc_chain_id_mismatch` token), and an RPC that does not answer in
# time serves Arc and logs `arc_rpc_chain_id_unverified`. Through 2.39.1 both
# were an error that stopped the whole process at startup.
#
# Filter-pattern note: substrings, as in alerts-evm-stuck-tx.tf. This log
# group is ANSI-coloured and the colour codes split key=value tokens, so the
# match is on the message tokens, spelled without spaces; `?` makes it either.
resource "aws_cloudwatch_log_metric_filter" "evm_rpc_chain_id" {
  name           = "facilitator-evm-rpc-chain-id"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "?evm_rpc_chain_id_mismatch ?arc_rpc_chain_id_unverified"

  metric_transformation {
    name      = "EvmRpcChainIdAlert"
    namespace = "Facilitator/ChainRail"
    value     = "1"
    unit      = "Count"
    # No default_value: the check runs once per task start, so absent data is
    # the normal state and is treated as notBreaching below.
  }
}

# One line per affected network per task start. A single datapoint pages:
# there is no blip to ride out, the RPC either answers for the chain or not.
# The alarm returns to OK on the next quiet period, and fires again on the next
# task start if nothing was fixed.
resource "aws_cloudwatch_metric_alarm" "evm_rpc_chain_id" {
  alarm_name          = "facilitator-${var.environment}-evm-rpc-chain-id"
  namespace           = aws_cloudwatch_log_metric_filter.evm_rpc_chain_id.metric_transformation[0].namespace
  metric_name         = aws_cloudwatch_log_metric_filter.evm_rpc_chain_id.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  alarm_description = "An EVM RPC answered eth_chainId with a different chain than the facilitator signs for (log token evm_rpc_chain_id_mismatch, naming the network and both ids): Arc is then left out of /supported, any other network stays served and its payments fail on-chain until the RPC URL or the chain id in EvmChain::try_from is corrected. Or Arc is served without its RPC having answered eth_chainId at startup (arc_rpc_chain_id_unverified). Checked once per task start by src/chain_identity.rs."

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-evm-rpc-chain-id"
    Environment = var.environment
  }
}
