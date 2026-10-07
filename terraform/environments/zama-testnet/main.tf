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
#
# ON/OFF: every resource and data source below carries
# `count = local.zama_count`, so `enable_zama = false` (the default, decision
# 171, 2026-10-06) leaves this state EMPTY and `true` builds the stack exactly
# as it was. moved.tf carries the old un-indexed addresses to `[0]`. The
# facilitator stops advertising fhe-transfer through its OWN flag
# (`enable_zama` in terraform/environments/production) -- turn that one off
# and deploy it BEFORE destroying this stack, or /supported keeps advertising
# a scheme whose backend is gone. Order and the way back: README.md, "On/off".

terraform {
  # 1.1 for the moved blocks in moved.tf.
  required_version = ">= 1.1"
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

locals {
  # One switch for the whole stack. Every block below reads this and nothing
  # else, so "off" cannot leave a stray alarm, budget or DNS record behind.
  zama_count = var.enable_zama ? 1 : 0
}

# ============================================================================
# Data Sources
# ============================================================================

data "aws_caller_identity" "current" {
  count = local.zama_count
}

data "aws_region" "current" {
  count = local.zama_count
}

data "aws_route53_zone" "main" {
  count = local.zama_count

  name         = var.hosted_zone_name
  private_zone = false
}

# ============================================================================
# S3 Bucket for Lambda Artifacts
# ============================================================================

resource "aws_s3_bucket" "lambda_artifacts" {
  count = local.zama_count

  bucket = "zama-facilitator-artifacts-${data.aws_caller_identity.current[0].account_id}"

  # `enable_zama = false` has to be able to delete this bucket, and it holds
  # the Lambda package (versioned). Without this the destroy stops at
  # BucketNotEmpty halfway through the stack. Terraform destroys with the
  # value already in the STATE, so this only takes effect after one apply
  # with enable_zama = true -- see README.md, "On/off".
  force_destroy = true

  tags = {
    Name = "zama-facilitator-lambda-artifacts"
  }
}

resource "aws_s3_bucket_versioning" "lambda_artifacts" {
  count = local.zama_count

  bucket = aws_s3_bucket.lambda_artifacts[0].id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_public_access_block" "lambda_artifacts" {
  count = local.zama_count

  bucket = aws_s3_bucket.lambda_artifacts[0].id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ============================================================================
# Secrets Manager for RPC URL
# ============================================================================

resource "aws_secretsmanager_secret" "sepolia_rpc" {
  count = local.zama_count

  name        = "zama-facilitator-sepolia-rpc"
  description = "Ethereum Sepolia RPC URL for Zama facilitator (Infura/Alchemy)"

  # Deleted at once rather than scheduled for 30 days: a scheduled deletion
  # keeps the NAME reserved, so turning the stack back on within the window
  # would fail at CreateSecret. The value is a third-party testnet RPC URL;
  # turning the stack back on means putting one again (outputs.tf, step 3).
  # Read from the state at destroy time, like force_destroy above.
  recovery_window_in_days = 0

  tags = {
    Name    = "zama-facilitator-sepolia-rpc"
    Network = "ethereum-sepolia"
  }
}

# ============================================================================
# IAM Role for Lambda Execution
# ============================================================================

resource "aws_iam_role" "lambda_exec" {
  count = local.zama_count

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
  count = local.zama_count

  role       = aws_iam_role.lambda_exec[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

# Secrets Manager permissions (CRITICAL - required for RPC URL access)
resource "aws_iam_role_policy" "lambda_secrets" {
  count = local.zama_count

  name = "secrets-access"
  role = aws_iam_role.lambda_exec[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "secretsmanager:GetSecretValue",
        "secretsmanager:DescribeSecret"
      ]
      Resource = aws_secretsmanager_secret.sepolia_rpc[0].arn
    }]
  })
}

# ============================================================================
# CloudWatch Log Groups
# ============================================================================

resource "aws_cloudwatch_log_group" "lambda" {
  count = local.zama_count

  name              = "/aws/lambda/zama-facilitator-${var.environment}"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "zama-facilitator-lambda-logs"
  }
}

resource "aws_cloudwatch_log_group" "api_gw" {
  count = local.zama_count

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
  count = local.zama_count

  function_name = "zama-facilitator-${var.environment}"
  role          = aws_iam_role.lambda_exec[0].arn
  handler       = "handler.handler"
  runtime       = "nodejs20.x"
  memory_size   = var.lambda_memory_size
  timeout       = var.fhe_request_timeout_secs

  # Source code from S3 (uploaded via CI/CD or manual deployment)
  s3_bucket = aws_s3_bucket.lambda_artifacts[0].id
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
  count = var.enable_zama && var.enable_provisioned_concurrency ? 1 : 0

  function_name                     = aws_lambda_function.zama_facilitator[0].function_name
  provisioned_concurrent_executions = var.provisioned_concurrency_count
  qualifier                         = aws_lambda_function.zama_facilitator[0].version
}

# Lambda permission for API Gateway
resource "aws_lambda_permission" "api_gw" {
  count = local.zama_count

  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.zama_facilitator[0].function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.main[0].execution_arn}/*/*"
}

# ============================================================================
# API Gateway HTTP API (v2)
# ============================================================================

resource "aws_apigatewayv2_api" "main" {
  count = local.zama_count

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
  count = local.zama_count

  api_id                 = aws_apigatewayv2_api.main[0].id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.zama_facilitator[0].invoke_arn
  payload_format_version = "2.0"

  # Derived from the single source of truth, then CLAMPED: an HTTP API caps
  # integration timeout at 30s and AWS marks that quota "Can be increased: No".
  # Stated explicitly rather than left to the implicit default, so the ceiling
  # is visible in the code instead of only in a 504.
  #
  # Consequence while fhe_request_timeout_secs > 30: the caller gets a 504 at
  # 30s and the Lambda keeps running (and billing) until its own timeout. To
  # actually honour 90s, put a Lambda Function URL in front instead of this
  # HTTP API -- see docs/plans/zama-developer-program/02-MAINNET-READINESS.md.
  timeout_milliseconds = min(var.fhe_request_timeout_secs * 1000, 30000)
}

resource "aws_apigatewayv2_route" "default" {
  count = local.zama_count

  api_id    = aws_apigatewayv2_api.main[0].id
  route_key = "$default"
  target    = "integrations/${aws_apigatewayv2_integration.lambda[0].id}"
}

resource "aws_apigatewayv2_stage" "default" {
  count = local.zama_count

  api_id      = aws_apigatewayv2_api.main[0].id
  name        = "$default"
  auto_deploy = true

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.api_gw[0].arn
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
  count = local.zama_count

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
  # Keyed by domain name as before, so the existing instance keeps its
  # address; with the stack off there is no certificate and the map is empty.
  for_each = {
    for dvo in flatten(aws_acm_certificate.main[*].domain_validation_options) : dvo.domain_name => {
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
  zone_id         = data.aws_route53_zone.main[0].zone_id
}

resource "aws_acm_certificate_validation" "main" {
  count = local.zama_count

  certificate_arn         = aws_acm_certificate.main[0].arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]
}

resource "aws_apigatewayv2_domain_name" "main" {
  count = local.zama_count

  domain_name = var.domain_name

  domain_name_configuration {
    certificate_arn = aws_acm_certificate.main[0].arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }

  depends_on = [aws_acm_certificate_validation.main]

  tags = {
    Name = "zama-facilitator-custom-domain"
  }
}

resource "aws_apigatewayv2_api_mapping" "main" {
  count = local.zama_count

  api_id      = aws_apigatewayv2_api.main[0].id
  domain_name = aws_apigatewayv2_domain_name.main[0].id
  stage       = aws_apigatewayv2_stage.default[0].id
}

resource "aws_route53_record" "main" {
  count = local.zama_count

  zone_id = data.aws_route53_zone.main[0].zone_id
  name    = var.domain_name
  type    = "A"

  alias {
    name                   = aws_apigatewayv2_domain_name.main[0].domain_name_configuration[0].target_domain_name
    zone_id                = aws_apigatewayv2_domain_name.main[0].domain_name_configuration[0].hosted_zone_id
    evaluate_target_health = false
  }
}

# ============================================================================
# CloudWatch Alarms
# ============================================================================

# Lambda invocation errors
resource "aws_cloudwatch_metric_alarm" "lambda_errors" {
  count = local.zama_count

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
    FunctionName = aws_lambda_function.zama_facilitator[0].function_name
  }

  tags = {
    Name = "zama-facilitator-lambda-errors-alarm"
  }
}

# Lambda duration approaching timeout
resource "aws_cloudwatch_metric_alarm" "lambda_duration" {
  count = local.zama_count

  alarm_name          = "zama-facilitator-lambda-duration-${var.environment}"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = "2"
  metric_name         = "Duration"
  namespace           = "AWS/Lambda"
  period              = "300"
  statistic           = "Average"
  threshold           = var.fhe_request_timeout_secs * 1000 * 0.8 # 80% of timeout (milliseconds)
  alarm_description   = "Lambda duration approaching timeout (${var.fhe_request_timeout_secs}s)"
  treat_missing_data  = "notBreaching"

  dimensions = {
    FunctionName = aws_lambda_function.zama_facilitator[0].function_name
  }

  tags = {
    Name = "zama-facilitator-lambda-duration-alarm"
  }
}

# API Gateway 5xx errors
resource "aws_cloudwatch_metric_alarm" "api_5xx_errors" {
  count = local.zama_count

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
    ApiId = aws_apigatewayv2_api.main[0].id
  }

  tags = {
    Name = "zama-facilitator-api-5xx-alarm"
  }
}

# ============================================================================
# AWS Budget Alert
# ============================================================================

resource "aws_budgets_budget" "zama_facilitator" {
  count = local.zama_count

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
