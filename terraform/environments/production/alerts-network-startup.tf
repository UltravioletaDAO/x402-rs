# Alarm for a network whose startup probe did not pass: an EVM RPC that answers
# for a different chain than the one the facilitator signs for, an Arc RPC that
# did not answer, or a native Hedera ledger whose health check failed at
# startup. None of them takes the network out of /supported.
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
# once per task start and logs `evm_rpc_chain_id_mismatch` when it differs. It
# is an alert and never a refusal, Arc included: the network stays in
# /supported, because a refusal would have switched both testnets off over a
# number in our own table. A wrong RPC does not settle on the wrong chain -- the
# token's domain is checked on-chain, and Arc recovers every signature locally
# under its configured chain id -- so the page is for a human to fix the RPC or
# the table. `/health/ready` reports the network `down` with
# `rpc_chain_id_mismatch` for as long as it lasts.
#
# An Arc RPC that does not answer at startup logs `arc_rpc_chain_id_unverified`
# and is asked again every 30-60 s until it gives a verdict. Native Hedera that
# fails its health check at startup logs `hedera_health_failed_at_startup`,
# stays in /supported and is checked again every 30-60 s; `hedera_health_recovered`
# (not counted here) says when it passed. Through 2.39.1 all three were an error
# that stopped the whole process, for every network, over one network's probe;
# through 2.39.3 an Arc mismatch or a failed Hedera check left the network out
# of /supported until the next deploy.
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

  alarm_description = "An EVM RPC answered eth_chainId with a different chain than the facilitator signs for (log token evm_rpc_chain_id_mismatch, naming the network and both ids): the network stays served and its payments fail until the RPC URL or the chain id in EvmChain::try_from is corrected; /health/ready reports it down with rpc_chain_id_mismatch. Or Arc's RPC did not answer eth_chainId at startup (arc_rpc_chain_id_unverified); it is asked again every 30-60 s. Or native Hedera failed its health check at startup (hedera_health_failed_at_startup): it stays served, /health/ready gives the reason, and the check runs again every 30-60 s until hedera_health_recovered. Logged once per task start (src/chain_identity.rs, src/chain/hedera/mod.rs)."

  alarm_actions = [aws_sns_topic.alerts.arn]
  ok_actions    = [aws_sns_topic.alerts.arn]

  tags = {
    Name        = "facilitator-${var.environment}-network-startup-probe"
    Environment = var.environment
  }
}
