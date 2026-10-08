# Terraform Outputs for Zama Facilitator

output "api_gateway_url" {
  description = "API Gateway default invoke URL"
  value       = one(aws_apigatewayv2_api.main[*].api_endpoint)
}

output "custom_domain_url" {
  description = "Custom domain URL for Zama facilitator"
  value       = var.enable_zama ? "https://${var.domain_name}" : null
}

output "lambda_function_name" {
  description = "Lambda function name"
  value       = one(aws_lambda_function.zama_facilitator[*].function_name)
}

output "lambda_function_arn" {
  description = "Lambda function ARN"
  value       = one(aws_lambda_function.zama_facilitator[*].arn)
}

output "s3_bucket" {
  description = "S3 bucket for Lambda deployment artifacts"
  value       = one(aws_s3_bucket.lambda_artifacts[*].id)
}

output "s3_bucket_arn" {
  description = "S3 bucket ARN"
  value       = one(aws_s3_bucket.lambda_artifacts[*].arn)
}

output "cloudwatch_log_group_lambda" {
  description = "CloudWatch log group for Lambda function"
  value       = one(aws_cloudwatch_log_group.lambda[*].name)
}

output "cloudwatch_log_group_api" {
  description = "CloudWatch log group for API Gateway"
  value       = one(aws_cloudwatch_log_group.api_gw[*].name)
}

output "secret_arn_sepolia_rpc" {
  description = "Secrets Manager ARN for Sepolia RPC URL"
  value       = one(aws_secretsmanager_secret.sepolia_rpc[*].arn)
}

output "fhe_request_timeout" {
  description = <<-EOT
    The effective timeout at each hop, so drift is readable without opening
    three files. `effective_end_to_end_secs` is what a caller can really wait
    for -- the API Gateway HTTP API caps at 30s and that quota is not
    increasable, so it is the binding constraint whenever it is below
    `configured_secs`.
  EOT
  value = var.enable_zama ? {
    configured_secs           = var.fhe_request_timeout_secs
    lambda_secs               = aws_lambda_function.zama_facilitator[0].timeout
    api_gateway_secs          = aws_apigatewayv2_integration.lambda[0].timeout_milliseconds / 1000
    effective_end_to_end_secs = min(var.fhe_request_timeout_secs, aws_apigatewayv2_integration.lambda[0].timeout_milliseconds / 1000)
    rust_proxy_env_var        = "FHE_PROXY_TIMEOUT_SECS (set in terraform/environments/production)"
  } : null
}

output "iam_role_lambda_exec" {
  description = "IAM role ARN for Lambda execution"
  value       = one(aws_iam_role.lambda_exec[*].arn)
}

output "route53_record_fqdn" {
  description = "Route53 FQDN for custom domain"
  value       = one(aws_route53_record.main[*].fqdn)
}

output "acm_certificate_arn" {
  description = "ACM certificate ARN"
  value       = one(aws_acm_certificate.main[*].arn)
}

output "deployment_instructions" {
  description = "Next steps for deployment"
  value = var.enable_zama ? (<<-EOT

    Zama Facilitator Infrastructure Created Successfully!

    Next steps:
    1. Upload Lambda deployment package:
       aws s3 cp handler.zip s3://${aws_s3_bucket.lambda_artifacts[0].id}/${var.lambda_s3_key}

    2. Update Lambda function code:
       aws lambda update-function-code \
         --function-name ${aws_lambda_function.zama_facilitator[0].function_name} \
         --s3-bucket ${aws_s3_bucket.lambda_artifacts[0].id} \
         --s3-key ${var.lambda_s3_key}

    3. Store Sepolia RPC URL in Secrets Manager:
       aws secretsmanager put-secret-value \
         --secret-id ${aws_secretsmanager_secret.sepolia_rpc[0].name} \
         --secret-string '{"url":"https://sepolia.infura.io/v3/YOUR_API_KEY"}'

    4. Test the health endpoint:
       curl https://${var.domain_name}/health

    5. Monitor logs:
       aws logs tail ${aws_cloudwatch_log_group.lambda[0].name} --follow

    Custom Domain URL: https://${var.domain_name}
    API Gateway URL: ${aws_apigatewayv2_api.main[0].api_endpoint}

  EOT
  ) : "enable_zama is false: this stack deploys nothing. How to turn it back on: README.md, On/off."
}
