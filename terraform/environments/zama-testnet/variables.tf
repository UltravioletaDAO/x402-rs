# Terraform Variables for Zama Facilitator (Testnet)

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-2"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "testnet"
}

variable "domain_name" {
  description = "Custom domain for Zama facilitator"
  type        = string
  default     = "zama-facilitator.ultravioletadao.xyz"
}

variable "hosted_zone_name" {
  description = "Route53 hosted zone name (must already exist)"
  type        = string
  default     = "ultravioletadao.xyz"
}

# ============================================================================
# Lambda Configuration
# ============================================================================

variable "lambda_memory_size" {
  description = "Lambda function memory in MB (1024 MB for FHE operations)"
  type        = number
  default     = 1024

  validation {
    condition     = var.lambda_memory_size >= 512 && var.lambda_memory_size <= 10240
    error_message = "Lambda memory must be between 512 and 10240 MB."
  }
}

variable "lambda_timeout" {
  description = "Lambda function timeout in seconds (30s for FHE decryption)"
  type        = number
  default     = 30

  validation {
    condition     = var.lambda_timeout >= 3 && var.lambda_timeout <= 900
    error_message = "Lambda timeout must be between 3 and 900 seconds."
  }
}

variable "lambda_s3_key" {
  description = "S3 key for Lambda deployment package (uploaded via CI/CD)"
  type        = string
  default     = "handler.zip"
}

variable "enable_provisioned_concurrency" {
  description = "Enable provisioned concurrency to mitigate cold starts (costs ~$8/month)"
  type        = bool
  default     = true
}

variable "provisioned_concurrency_count" {
  description = "Number of provisioned concurrent executions (1 instance recommended for testnet)"
  type        = number
  default     = 1

  validation {
    condition     = var.provisioned_concurrency_count >= 0 && var.provisioned_concurrency_count <= 10
    error_message = "Provisioned concurrency must be between 0 and 10."
  }
}

# ============================================================================
# CORS and Networking
# ============================================================================

variable "cors_origins" {
  description = "Comma-separated list of allowed CORS origins"
  type        = string
  default     = "https://ultravioletadao.xyz,http://localhost:3000"
}

# ============================================================================
# CloudWatch and Logging
# ============================================================================

variable "log_retention_days" {
  description = "CloudWatch log retention in days"
  type        = number
  default     = 14

  validation {
    condition     = contains([1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1827, 3653], var.log_retention_days)
    error_message = "Log retention must be a valid CloudWatch retention period."
  }
}

# ============================================================================
# CloudWatch Alarms
# ============================================================================

variable "lambda_error_threshold" {
  description = "Number of Lambda errors to trigger alarm"
  type        = number
  default     = 5
}

variable "api_5xx_threshold" {
  description = "Number of API Gateway 5xx errors to trigger alarm"
  type        = number
  default     = 10
}

# ============================================================================
# Cost Management
# ============================================================================

variable "budget_limit" {
  description = "Monthly budget limit in USD"
  type        = string
  default     = "20"
}

variable "budget_alert_emails" {
  description = "Email addresses for budget alerts"
  type        = list(string)
  default     = ["alerts@ultravioletadao.xyz"]
}
