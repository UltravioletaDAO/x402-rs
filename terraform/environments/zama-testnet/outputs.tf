# Terraform Outputs for Zama Facilitator

output "api_gateway_url" {
  description = "API Gateway default invoke URL"
  value       = aws_apigatewayv2_api.main.api_endpoint
}

output "custom_domain_url" {
  description = "Custom domain URL for Zama facilitator"
  value       = "https://${var.domain_name}"
}

output "lambda_function_name" {
  description = "Lambda function name"
  value       = aws_lambda_function.zama_facilitator.function_name
}

output "lambda_function_arn" {
  description = "Lambda function ARN"
  value       = aws_lambda_function.zama_facilitator.arn
}

output "s3_bucket" {
  description = "S3 bucket for Lambda deployment artifacts"
  value       = aws_s3_bucket.lambda_artifacts.id
}

output "s3_bucket_arn" {
  description = "S3 bucket ARN"
  value       = aws_s3_bucket.lambda_artifacts.arn
}

output "cloudwatch_log_group_lambda" {
  description = "CloudWatch log group for Lambda function"
  value       = aws_cloudwatch_log_group.lambda.name
}

output "cloudwatch_log_group_api" {
  description = "CloudWatch log group for API Gateway"
  value       = aws_cloudwatch_log_group.api_gw.name
}

output "secret_arn_sepolia_rpc" {
  description = "Secrets Manager ARN for Sepolia RPC URL"
  value       = aws_secretsmanager_secret.sepolia_rpc.arn
}

output "iam_role_lambda_exec" {
  description = "IAM role ARN for Lambda execution"
  value       = aws_iam_role.lambda_exec.arn
}

output "route53_record_fqdn" {
  description = "Route53 FQDN for custom domain"
  value       = aws_route53_record.main.fqdn
}

output "acm_certificate_arn" {
  description = "ACM certificate ARN"
  value       = aws_acm_certificate.main.arn
}

output "deployment_instructions" {
  description = "Next steps for deployment"
  value       = <<-EOT

    Zama Facilitator Infrastructure Created Successfully!

    Next steps:
    1. Upload Lambda deployment package:
       aws s3 cp handler.zip s3://${aws_s3_bucket.lambda_artifacts.id}/${var.lambda_s3_key}

    2. Update Lambda function code:
       aws lambda update-function-code \
         --function-name ${aws_lambda_function.zama_facilitator.function_name} \
         --s3-bucket ${aws_s3_bucket.lambda_artifacts.id} \
         --s3-key ${var.lambda_s3_key}

    3. Store Sepolia RPC URL in Secrets Manager:
       aws secretsmanager put-secret-value \
         --secret-id ${aws_secretsmanager_secret.sepolia_rpc.name} \
         --secret-string '{"url":"https://sepolia.infura.io/v3/YOUR_API_KEY"}'

    4. Test the health endpoint:
       curl https://${var.domain_name}/health

    5. Monitor logs:
       aws logs tail ${aws_cloudwatch_log_group.lambda.name} --follow

    Custom Domain URL: https://${var.domain_name}
    API Gateway URL: ${aws_apigatewayv2_api.main.api_endpoint}

  EOT
}
