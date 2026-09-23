# Alarm for a network whose startup probe did not pass: an EVM RPC that answers
# for a different chain than the one the facilitator signs for, an Arc RPC served
# without having answered, or a native Hedera ledger left out because its health
# check failed at startup.
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
# time serves Arc and logs `arc_rpc_chain_id_unverified`. Native Hedera that
# fails its health check at startup is left out of /supported and logs
# `hedera_health_failed_at_startup`; recovery of payments admitted before the
# restart still runs. Through 2.39.1 all three were an error that stopped the
# whole process, for every network, over one network's probe.
#
# Filter-pattern note: substrings, as in alerts-evm-stuck-tx.tf. This log
# group is ANSI-coloured and the colour codes split key=value tokens, so the
# match is on the message tokens, spelled without spaces; `?` makes it either.
resource "aws_cloudwatch_log_metric_filter" "network_startup_probe" {
  name           = "facilitator-network-startup-probe"
  log_group_name = aws_cloudwatch_log_group.facilitator.name
  pattern        = "?evm_rpc_chain_id_mismatch ?arc_rpc_chain_id_unverified ?hedera_health_failed_at_startup"

  metric_transformation {
    name      = "NetworkStartupProbeAlert"
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
resource "aws_cloudwatch_metric_alarm" "network_startup_probe" {
  alarm_name          = "facilitator-${var.environment}-network-startup-probe"
  namespace           = aws_cloudwatch_log_metric_filter.network_startup_probe.metric_transformation[0].namespace
  metric_name         = aws_cloudwatch_log_metric_filter.network_startup_probe.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  alarm_description = "An EVM RPC answered eth_chainId with a different chain than the facilitator signs for (log token evm_rpc_chain_id_mismatch, naming the network and both ids): Arc is then left out of /supported, any other network stays served and its payments fail on-chain until the RPC URL or the chain id in EvmChain::try_from is corrected. Or Arc is served without its RPC having answered eth_chainId at startup (arc_rpc_chain_id_unverified). Or native Hedera failed its health check at startup and is not served (hedera_health_failed_at_startup; recovery of admitted payments still runs). Checked once per task start (src/chain_identity.rs, src/chain/hedera/mod.rs)."

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-network-startup-probe"
    Environment = var.environment
  }
}
