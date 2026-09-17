# Native Hedera admission and recovery. Kept separate from the best-effort
# transaction index: a failed write here MUST prevent sponsorship/submission.
variable "hedera_enabled_testnet" {
  type    = bool
  default = false
}
variable "hedera_enabled_mainnet" {
  type    = bool
  default = false
}
variable "hedera_daily_budget_tinybars_testnet" {
  description = "UTC-day ceiling on reserved MAX transaction fees, shared by all replicas"
  type        = number
  default     = 1000000000
}
variable "hedera_daily_budget_tinybars_mainnet" {
  description = "Explicit mainnet ceiling; zero prevents activation until a budget is set"
  type        = number
  default     = 0
}

resource "aws_dynamodb_table" "hedera_settlements" {
  name         = "facilitator-hedera-settlements"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "id"
  attribute {
    name = "id"
    type = "S"
  }
  attribute {
    name = "recovery_network"
    type = "S"
  }
  attribute {
    name = "recovery_lease"
    type = "N"
  }
  global_secondary_index {
    name            = "recovery"
    hash_key        = "recovery_network"
    range_key       = "recovery_lease"
    projection_type = "ALL"
  }
  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }
  point_in_time_recovery {
    enabled = true
  }
  tags = {
    Name        = "facilitator-hedera-settlements"
    Environment = var.environment
  }
}

resource "aws_iam_role_policy" "hedera_settlement_access" {
  name = "HederaSettlementAccess"
  role = aws_iam_role.ecs_task.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:UpdateItem", "dynamodb:DescribeTable", "dynamodb:Query"]
      Resource = [aws_dynamodb_table.hedera_settlements.arn, "${aws_dynamodb_table.hedera_settlements.arn}/index/recovery"]
    }]
  })
}

# The keys already exist; account_id is injected only once the corresponding
# network is enabled. A null field must never prevent unrelated chains booting.
data "aws_secretsmanager_secret" "hedera_testnet" {
  name = "facilitator-hedera-testnet-keypair"
}
data "aws_secretsmanager_secret" "hedera_mainnet" {
  name = "facilitator-hedera-mainnet-keypair"
}
locals {
  hedera_environment = [
    { name = "HEDERA_ENABLED_TESTNET", value = tostring(var.hedera_enabled_testnet) },
    { name = "HEDERA_ENABLED_MAINNET", value = tostring(var.hedera_enabled_mainnet) },
    { name = "HEDERA_DAILY_BUDGET_TINYBARS_TESTNET", value = tostring(var.hedera_daily_budget_tinybars_testnet) },
    { name = "HEDERA_DAILY_BUDGET_TINYBARS_MAINNET", value = tostring(var.hedera_daily_budget_tinybars_mainnet) },
    { name = "HEDERA_SETTLEMENT_TABLE_NAME", value = aws_dynamodb_table.hedera_settlements.name },
  ]
  hedera_secrets = concat(
    var.hedera_enabled_testnet ? [
      { name = "HEDERA_PRIVATE_KEY_TESTNET", valueFrom = "${data.aws_secretsmanager_secret.hedera_testnet.arn}:private_key::" },
      { name = "HEDERA_ACCOUNT_ID_TESTNET", valueFrom = "${data.aws_secretsmanager_secret.hedera_testnet.arn}:account_id::" },
    ] : [],
    var.hedera_enabled_mainnet ? [
      { name = "HEDERA_PRIVATE_KEY_MAINNET", valueFrom = "${data.aws_secretsmanager_secret.hedera_mainnet.arn}:private_key::" },
      { name = "HEDERA_ACCOUNT_ID_MAINNET", valueFrom = "${data.aws_secretsmanager_secret.hedera_mainnet.arn}:account_id::" },
    ] : [],
  )
}

# Public numeric IDs for balance monitoring only. Signers still come from the
# existing Secrets Manager records and verify ownership against Mirror.
variable "hedera_account_id_testnet" {
  type    = string
  default = ""
}
variable "hedera_account_id_mainnet" {
  type    = string
  default = ""
}
locals {
  hedera_balance_environment = merge(
    var.hedera_enabled_testnet ? { HEDERA_ACCOUNT_ID_TESTNET = var.hedera_account_id_testnet } : {},
    var.hedera_enabled_mainnet ? { HEDERA_ACCOUNT_ID_MAINNET = var.hedera_account_id_mainnet } : {},
  )
}
