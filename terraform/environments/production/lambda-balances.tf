# ============================================================================
# Lambda Function for Wallet Balances API
# ============================================================================
# This Lambda fetches wallet balances from all networks and returns them
# via API Gateway. RPC URLs with API keys are stored in Secrets Manager.
#
# Endpoint: https://facilitator.ultravioletadao.xyz/api/balances
# ============================================================================

# ============================================================================
# Lambda IAM Role
# ============================================================================

resource "aws_iam_role" "balances_lambda" {
  name = "facilitator-${var.environment}-balances-lambda"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "lambda.amazonaws.com"
        }
      }
    ]
  })

  tags = {
    Name = "facilitator-${var.environment}-balances-lambda"
  }
}

# Basic Lambda execution policy (CloudWatch Logs)
resource "aws_iam_role_policy_attachment" "balances_lambda_basic" {
  role       = aws_iam_role.balances_lambda.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

# Policy for accessing Secrets Manager (RPC URLs with API keys)
resource "aws_iam_role_policy" "balances_lambda_secrets" {
  name = "secrets-access"
  role = aws_iam_role.balances_lambda.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = [
          "arn:aws:secretsmanager:${data.aws_region.current.name}:${data.aws_caller_identity.current.account_id}:secret:facilitator-rpc-*"
        ]
      }
    ]
  })
}

# ============================================================================
# Lambda Function
# ============================================================================

# Create deployment package
data "archive_file" "balances_lambda" {
  type        = "zip"
  source_dir  = "${path.module}/../../../lambda/balances"
  output_path = "${path.module}/balances_lambda.zip"
}

resource "aws_lambda_function" "balances" {
  filename         = data.archive_file.balances_lambda.output_path
  source_code_hash = data.archive_file.balances_lambda.output_base64sha256
  function_name    = "facilitator-${var.environment}-balances"
  role             = aws_iam_role.balances_lambda.arn
  handler          = "handler.lambda_handler"
  runtime          = "python3.12"
  timeout          = 30
  memory_size      = 256

  environment {
    variables = {
      # Public RPC URLs (no API keys)
      RPC_URL_BASE       = "https://mainnet.base.org"
      RPC_URL_AVALANCHE  = "https://avalanche-c-chain-rpc.publicnode.com"
      RPC_URL_CELO       = "https://rpc.celocolombia.org"
      RPC_URL_HYPEREVM   = "https://rpc.hyperliquid.xyz/evm"
      RPC_URL_POLYGON    = "https://polygon.drpc.org"
      RPC_URL_OPTIMISM   = "https://mainnet.optimism.io"
      RPC_URL_ETHEREUM   = "https://ethereum-rpc.publicnode.com"
      RPC_URL_ARBITRUM   = "https://arb1.arbitrum.io/rpc"
      RPC_URL_UNICHAIN   = "https://unichain-rpc.publicnode.com"
      RPC_URL_MONAD      = "https://rpc.monad.xyz"
      RPC_URL_BSC        = "https://bsc-dataseed.binance.org"
      RPC_URL_SUI        = "https://fullnode.mainnet.sui.io:443"
      # Private RPC URLs (with API keys) - override via Secrets Manager
      # RPC_URL_SOLANA will be set from secretsmanager
    }
  }

  tags = {
    Name = "facilitator-${var.environment}-balances"
  }
}

# CloudWatch Log Group for Lambda
resource "aws_cloudwatch_log_group" "balances_lambda" {
  name              = "/aws/lambda/facilitator-${var.environment}-balances"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "facilitator-${var.environment}-balances-lambda"
  }
}

# ============================================================================
# API Gateway HTTP API
# ============================================================================

resource "aws_apigatewayv2_api" "balances" {
  name          = "facilitator-${var.environment}-balances"
  protocol_type = "HTTP"

  cors_configuration {
    allow_origins = ["*"]
    allow_methods = ["GET", "OPTIONS"]
    allow_headers = ["Content-Type"]
    max_age       = 86400
  }

  tags = {
    Name = "facilitator-${var.environment}-balances-api"
  }
}

# Lambda integration
resource "aws_apigatewayv2_integration" "balances" {
  api_id             = aws_apigatewayv2_api.balances.id
  integration_type   = "AWS_PROXY"
  integration_uri    = aws_lambda_function.balances.invoke_arn
  integration_method = "POST"
}

# Route: GET /balances
resource "aws_apigatewayv2_route" "balances" {
  api_id    = aws_apigatewayv2_api.balances.id
  route_key = "GET /balances"
  target    = "integrations/${aws_apigatewayv2_integration.balances.id}"
}

# Default stage with auto-deploy
resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.balances.id
  name        = "$default"
  auto_deploy = true

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.balances_api.arn
    format = jsonencode({
      requestId      = "$context.requestId"
      ip             = "$context.identity.sourceIp"
      requestTime    = "$context.requestTime"
      httpMethod     = "$context.httpMethod"
      routeKey       = "$context.routeKey"
      status         = "$context.status"
      responseLength = "$context.responseLength"
      latency        = "$context.integrationLatency"
    })
  }

  tags = {
    Name = "facilitator-${var.environment}-balances-api-stage"
  }
}

# CloudWatch Log Group for API Gateway
resource "aws_cloudwatch_log_group" "balances_api" {
  name              = "/aws/apigateway/facilitator-${var.environment}-balances"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "facilitator-${var.environment}-balances-api"
  }
}

# Lambda permission for API Gateway
resource "aws_lambda_permission" "balances_api" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.balances.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.balances.execution_arn}/*/*"
}

# ============================================================================
# ALB Integration - Route /api/balances through existing ALB
# ============================================================================
# This allows the frontend to call /api/balances on the same domain
# instead of needing a separate API Gateway URL.

# Target group for Lambda (no health check needed for Lambda targets)
resource "aws_lb_target_group" "balances_lambda" {
  name        = "facilitator-${var.environment}-balances"
  target_type = "lambda"

  tags = {
    Name = "facilitator-${var.environment}-balances-tg"
  }
}

# Attach Lambda to target group
resource "aws_lb_target_group_attachment" "balances_lambda" {
  target_group_arn = aws_lb_target_group.balances_lambda.arn
  target_id        = aws_lambda_function.balances.arn
  depends_on       = [aws_lambda_permission.balances_alb]
}

# Lambda permission for ALB
resource "aws_lambda_permission" "balances_alb" {
  statement_id  = "AllowALBInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.balances.function_name
  principal     = "elasticloadbalancing.amazonaws.com"
  source_arn    = aws_lb_target_group.balances_lambda.arn
}

# Listener rule to route /api/balances to Lambda
resource "aws_lb_listener_rule" "balances_api" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 10

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.balances_lambda.arn
  }

  condition {
    path_pattern {
      values = ["/api/balances"]
    }
  }

  tags = {
    Name = "facilitator-${var.environment}-balances-rule"
  }
}

# ============================================================================
# Outputs
# ============================================================================

output "balances_api_url" {
  description = "URL for the balances API endpoint"
  value       = "${aws_apigatewayv2_api.balances.api_endpoint}/balances"
}

output "balances_lambda_arn" {
  description = "ARN of the balances Lambda function"
  value       = aws_lambda_function.balances.arn
}
