# ============================================================================
# Zama Facilitator (Testnet) - Lambda + API Gateway Infrastructure
# ============================================================================
# Deploys x402-zama FHE payment facilitator for Ethereum Sepolia testnet
# Cost estimate: ~$15/month (Lambda + Provisioned Concurrency + API Gateway)
#
# Architecture:
# - Lambda function (Node.js 20.x, 1GB RAM, 30s timeout)
# - API Gateway HTTP API (v2) with custom domain
# - CloudWatch Logs (14 day retention)
# - Secrets Manager for RPC URLs
# - Provisioned Concurrency (1 instance) to mitigate cold starts

terraform {
  required_version = ">= 1.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project     = "x402-zama-facilitator"
      Environment = var.environment
      ManagedBy   = "terraform"
      Owner       = "ultravioleta-dao"
      Service     = "fhe-payments"
    }
  }
}

# ============================================================================
# Data Sources
# ============================================================================

data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

data "aws_route53_zone" "main" {
  name         = var.hosted_zone_name
  private_zone = false
}

# ============================================================================
# S3 Bucket for Lambda Artifacts
# ============================================================================

resource "aws_s3_bucket" "lambda_artifacts" {
  bucket = "zama-facilitator-artifacts-${data.aws_caller_identity.current.account_id}"

  tags = {
    Name = "zama-facilitator-lambda-artifacts"
  }
}

resource "aws_s3_bucket_versioning" "lambda_artifacts" {
  bucket = aws_s3_bucket.lambda_artifacts.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_public_access_block" "lambda_artifacts" {
  bucket = aws_s3_bucket.lambda_artifacts.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ============================================================================
# Secrets Manager for RPC URL
# ============================================================================

resource "aws_secretsmanager_secret" "sepolia_rpc" {
  name        = "zama-facilitator-sepolia-rpc"
  description = "Ethereum Sepolia RPC URL for Zama facilitator (Infura/Alchemy)"

  tags = {
    Name    = "zama-facilitator-sepolia-rpc"
    Network = "ethereum-sepolia"
  }
}

# ============================================================================
# IAM Role for Lambda Execution
# ============================================================================

resource "aws_iam_role" "lambda_exec" {
  name = "zama-facilitator-lambda-${var.environment}"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = {
        Service = "lambda.amazonaws.com"
      }
    }]
  })

  tags = {
    Name = "zama-facilitator-lambda-execution-role"
  }
}

# CloudWatch Logs permissions
resource "aws_iam_role_policy_attachment" "lambda_logs" {
  role       = aws_iam_role.lambda_exec.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

# Secrets Manager permissions (CRITICAL - required for RPC URL access)
resource "aws_iam_role_policy" "lambda_secrets" {
  name = "secrets-access"
  role = aws_iam_role.lambda_exec.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "secretsmanager:GetSecretValue",
        "secretsmanager:DescribeSecret"
      ]
      Resource = aws_secretsmanager_secret.sepolia_rpc.arn
    }]
  })
}

# ============================================================================
# CloudWatch Log Groups
# ============================================================================

resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/zama-facilitator-${var.environment}"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "zama-facilitator-lambda-logs"
  }
}

resource "aws_cloudwatch_log_group" "api_gw" {
  name              = "/aws/api-gw/zama-facilitator-${var.environment}"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "zama-facilitator-api-gateway-logs"
  }
}

# ============================================================================
# Lambda Function
# ============================================================================

resource "aws_lambda_function" "zama_facilitator" {
  function_name = "zama-facilitator-${var.environment}"
  role          = aws_iam_role.lambda_exec.arn
  handler       = "handler.handler"
  runtime       = "nodejs20.x"
  memory_size   = var.lambda_memory_size
  timeout       = var.lambda_timeout

  # Source code from S3 (uploaded via CI/CD or manual deployment)
  s3_bucket = aws_s3_bucket.lambda_artifacts.id
  s3_key    = var.lambda_s3_key

  environment {
    variables = {
      NODE_ENV     = "production"
      CORS_ORIGINS = var.cors_origins
      # SEPOLIA_RPC_URL is loaded from Secrets Manager at runtime
    }
  }

  depends_on = [
    aws_cloudwatch_log_group.lambda,
    aws_iam_role_policy.lambda_secrets
  ]

  tags = {
    Name = "zama-facilitator-lambda"
  }
}

# Provisioned Concurrency (mitigate cold starts)
resource "aws_lambda_provisioned_concurrency_config" "zama" {
  count = var.enable_provisioned_concurrency ? 1 : 0

  function_name                     = aws_lambda_function.zama_facilitator.function_name
  provisioned_concurrent_executions = var.provisioned_concurrency_count
  qualifier                         = aws_lambda_function.zama_facilitator.version
}

# Lambda permission for API Gateway
resource "aws_lambda_permission" "api_gw" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.zama_facilitator.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.main.execution_arn}/*/*"
}

# ============================================================================
# API Gateway HTTP API (v2)
# ============================================================================

resource "aws_apigatewayv2_api" "main" {
  name          = "zama-facilitator-${var.environment}"
  protocol_type = "HTTP"
  description   = "HTTP API for Zama FHE payment facilitator (x402-zama)"

  cors_configuration {
    allow_origins = split(",", var.cors_origins)
    allow_methods = ["GET", "POST", "OPTIONS"]
    allow_headers = ["Content-Type", "Authorization", "x-payment"]
    max_age       = 300
  }

  tags = {
    Name = "zama-facilitator-api"
  }
}

resource "aws_apigatewayv2_integration" "lambda" {
  api_id                 = aws_apigatewayv2_api.main.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.zama_facilitator.invoke_arn
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "default" {
  api_id    = aws_apigatewayv2_api.main.id
  route_key = "$default"
  target    = "integrations/${aws_apigatewayv2_integration.lambda.id}"
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.main.id
  name        = "$default"
  auto_deploy = true

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.api_gw.arn
    format = jsonencode({
      requestId      = "$context.requestId"
      ip             = "$context.identity.sourceIp"
      requestTime    = "$context.requestTime"
      httpMethod     = "$context.httpMethod"
      routeKey       = "$context.routeKey"
      status         = "$context.status"
      responseLength = "$context.responseLength"
      errorMessage   = "$context.error.message"
    })
  }

  tags = {
    Name = "zama-facilitator-api-stage"
  }
}

# ============================================================================
# Custom Domain (ACM + Route53)
# ============================================================================

resource "aws_acm_certificate" "main" {
  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }

  tags = {
    Name = "zama-facilitator-certificate"
  }
}

resource "aws_route53_record" "cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.main.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }

  allow_overwrite = true
  name            = each.value.name
  records         = [each.value.record]
  ttl             = 60
  type            = each.value.type
  zone_id         = data.aws_route53_zone.main.zone_id
}

resource "aws_acm_certificate_validation" "main" {
  certificate_arn         = aws_acm_certificate.main.arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]
}

resource "aws_apigatewayv2_domain_name" "main" {
  domain_name = var.domain_name

  domain_name_configuration {
    certificate_arn = aws_acm_certificate.main.arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }

  depends_on = [aws_acm_certificate_validation.main]

  tags = {
    Name = "zama-facilitator-custom-domain"
  }
}

resource "aws_apigatewayv2_api_mapping" "main" {
  api_id      = aws_apigatewayv2_api.main.id
  domain_name = aws_apigatewayv2_domain_name.main.id
  stage       = aws_apigatewayv2_stage.default.id
}

resource "aws_route53_record" "main" {
  zone_id = data.aws_route53_zone.main.zone_id
  name    = var.domain_name
  type    = "A"

  alias {
    name                   = aws_apigatewayv2_domain_name.main.domain_name_configuration[0].target_domain_name
    zone_id                = aws_apigatewayv2_domain_name.main.domain_name_configuration[0].hosted_zone_id
    evaluate_target_health = false
  }
}

# ============================================================================
# CloudWatch Alarms
# ============================================================================

# Lambda invocation errors
resource "aws_cloudwatch_metric_alarm" "lambda_errors" {
  alarm_name          = "zama-facilitator-lambda-errors-${var.environment}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = "2"
  metric_name         = "Errors"
  namespace           = "AWS/Lambda"
  period              = "300"
  statistic           = "Sum"
  threshold           = var.lambda_error_threshold
  alarm_description   = "Lambda function errors exceed threshold"
  treat_missing_data  = "notBreaching"

  dimensions = {
    FunctionName = aws_lambda_function.zama_facilitator.function_name
  }

  tags = {
    Name = "zama-facilitator-lambda-errors-alarm"
  }
}

# Lambda duration approaching timeout
resource "aws_cloudwatch_metric_alarm" "lambda_duration" {
  alarm_name          = "zama-facilitator-lambda-duration-${var.environment}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = "2"
  metric_name         = "Duration"
  namespace           = "AWS/Lambda"
  period              = "300"
  statistic           = "Average"
  threshold           = var.lambda_timeout * 1000 * 0.8 # 80% of timeout (milliseconds)
  alarm_description   = "Lambda duration approaching timeout (${var.lambda_timeout}s)"
  treat_missing_data  = "notBreaching"

  dimensions = {
    FunctionName = aws_lambda_function.zama_facilitator.function_name
  }

  tags = {
    Name = "zama-facilitator-lambda-duration-alarm"
  }
}

# API Gateway 5xx errors
resource "aws_cloudwatch_metric_alarm" "api_5xx_errors" {
  alarm_name          = "zama-facilitator-api-5xx-${var.environment}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = "2"
  metric_name         = "5XXError"
  namespace           = "AWS/ApiGateway"
  period              = "300"
  statistic           = "Sum"
  threshold           = var.api_5xx_threshold
  alarm_description   = "API Gateway 5xx errors exceed threshold"
  treat_missing_data  = "notBreaching"

  dimensions = {
    ApiId = aws_apigatewayv2_api.main.id
  }

  tags = {
    Name = "zama-facilitator-api-5xx-alarm"
  }
}

# ============================================================================
# AWS Budget Alert
# ============================================================================

resource "aws_budgets_budget" "zama_facilitator" {
  name         = "zama-facilitator-monthly"
  budget_type  = "COST"
  limit_amount = var.budget_limit
  limit_unit   = "USD"
  time_unit    = "MONTHLY"

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = 80
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_email_addresses = var.budget_alert_emails
  }

  cost_filter {
    name = "TagKeyValue"
    values = [
      "user:Project$x402-zama-facilitator"
    ]
  }

  tags = {
    Name = "zama-facilitator-budget"
  }
}
