# ============================================================================
# State moves for the enable_zama switch
# ============================================================================
# Every resource in main.tf gained `count = local.zama_count`, which moves
# its address from `aws_x.name` to `aws_x.name[0]`. Without these blocks the
# first plan after that change reads as "destroy everything, create
# everything" even with enable_zama = true. With them, `true` plans the moves
# and nothing else that the switch caused; `false` moves, then destroys `[0]`.
#
# Not listed: aws_lambda_provisioned_concurrency_config.zama (already counted)
# and aws_route53_record.cert_validation (for_each, keyed by domain name as
# before) -- their addresses did not change. Data sources need no move.
#
# Safe to delete once the state has been applied with this file in place.

moved {
  from = aws_s3_bucket.lambda_artifacts
  to   = aws_s3_bucket.lambda_artifacts[0]
}

moved {
  from = aws_s3_bucket_versioning.lambda_artifacts
  to   = aws_s3_bucket_versioning.lambda_artifacts[0]
}

moved {
  from = aws_s3_bucket_public_access_block.lambda_artifacts
  to   = aws_s3_bucket_public_access_block.lambda_artifacts[0]
}

moved {
  from = aws_secretsmanager_secret.sepolia_rpc
  to   = aws_secretsmanager_secret.sepolia_rpc[0]
}

moved {
  from = aws_iam_role.lambda_exec
  to   = aws_iam_role.lambda_exec[0]
}

moved {
  from = aws_iam_role_policy_attachment.lambda_logs
  to   = aws_iam_role_policy_attachment.lambda_logs[0]
}

moved {
  from = aws_iam_role_policy.lambda_secrets
  to   = aws_iam_role_policy.lambda_secrets[0]
}

moved {
  from = aws_cloudwatch_log_group.lambda
  to   = aws_cloudwatch_log_group.lambda[0]
}

moved {
  from = aws_cloudwatch_log_group.api_gw
  to   = aws_cloudwatch_log_group.api_gw[0]
}

moved {
  from = aws_lambda_function.zama_facilitator
  to   = aws_lambda_function.zama_facilitator[0]
}

moved {
  from = aws_lambda_permission.api_gw
  to   = aws_lambda_permission.api_gw[0]
}

moved {
  from = aws_apigatewayv2_api.main
  to   = aws_apigatewayv2_api.main[0]
}

moved {
  from = aws_apigatewayv2_integration.lambda
  to   = aws_apigatewayv2_integration.lambda[0]
}

moved {
  from = aws_apigatewayv2_route.default
  to   = aws_apigatewayv2_route.default[0]
}

moved {
  from = aws_apigatewayv2_stage.default
  to   = aws_apigatewayv2_stage.default[0]
}

moved {
  from = aws_acm_certificate.main
  to   = aws_acm_certificate.main[0]
}

moved {
  from = aws_acm_certificate_validation.main
  to   = aws_acm_certificate_validation.main[0]
}

moved {
  from = aws_apigatewayv2_domain_name.main
  to   = aws_apigatewayv2_domain_name.main[0]
}

moved {
  from = aws_apigatewayv2_api_mapping.main
  to   = aws_apigatewayv2_api_mapping.main[0]
}

moved {
  from = aws_route53_record.main
  to   = aws_route53_record.main[0]
}

moved {
  from = aws_cloudwatch_metric_alarm.lambda_errors
  to   = aws_cloudwatch_metric_alarm.lambda_errors[0]
}

moved {
  from = aws_cloudwatch_metric_alarm.lambda_duration
  to   = aws_cloudwatch_metric_alarm.lambda_duration[0]
}

moved {
  from = aws_cloudwatch_metric_alarm.api_5xx_errors
  to   = aws_cloudwatch_metric_alarm.api_5xx_errors[0]
}

moved {
  from = aws_budgets_budget.zama_facilitator
  to   = aws_budgets_budget.zama_facilitator[0]
}
