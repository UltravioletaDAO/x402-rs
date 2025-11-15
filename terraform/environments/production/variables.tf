# Terraform Variables for Facilitator Production Environment

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-2"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "production"
}

variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.1.0.0/16"
}

variable "availability_zones" {
  description = "Availability zones"
  type        = list(string)
  default     = ["us-east-2a", "us-east-2b"]
}

variable "use_fargate_spot" {
  description = "Use Fargate Spot for cost savings (false for facilitator - needs stability)"
  type        = bool
  default     = false
}

variable "use_nat_instance" {
  description = "Use NAT instance instead of NAT Gateway ($8/mo vs $32/mo)"
  type        = bool
  default     = true
}

variable "enable_vpc_endpoints" {
  description = "Enable VPC endpoints (costs $35/mo but reduces NAT data transfer)"
  type        = bool
  default     = false
}

variable "single_nat_gateway" {
  description = "Use single NAT gateway (true) or one per AZ (false)"
  type        = bool
  default     = true
}

variable "task_cpu" {
  description = "Fargate task CPU units (1024 = 1 vCPU)"
  type        = number
  default     = 1024
}

variable "task_memory" {
  description = "Fargate task memory in MB"
  type        = number
  default     = 2048
}

variable "desired_count" {
  description = "Desired number of tasks"
  type        = number
  default     = 1
}

variable "min_capacity" {
  description = "Minimum number of tasks for auto-scaling"
  type        = number
  default     = 1
}

variable "max_capacity" {
  description = "Maximum number of tasks for auto-scaling"
  type        = number
  default     = 3
}

variable "cpu_target_value" {
  description = "Target CPU utilization for auto-scaling"
  type        = number
  default     = 75
}

variable "memory_target_value" {
  description = "Target memory utilization for auto-scaling"
  type        = number
  default     = 80
}

variable "alb_idle_timeout" {
  description = "ALB idle timeout in seconds"
  type        = number
  default     = 180
}

variable "domain_name" {
  description = "Domain name for facilitator"
  type        = string
  default     = "facilitator.ultravioletadao.xyz"
}

variable "hosted_zone_name" {
  description = "Route53 hosted zone name"
  type        = string
  default     = "ultravioletadao.xyz"
}

variable "evm_secret_name" {
  description = "AWS Secrets Manager secret name for EVM private key"
  type        = string
  default     = "facilitator-evm-private-key"
}

variable "solana_secret_name" {
  description = "AWS Secrets Manager secret name for Solana keypair"
  type        = string
  default     = "facilitator-solana-keypair"
}

variable "quicknode_secret_name" {
  description = "AWS Secrets Manager secret name for QuickNode RPC (optional)"
  type        = string
  default     = "facilitator-quicknode-base-rpc"
}

variable "log_retention_days" {
  description = "CloudWatch log retention in days"
  type        = number
  default     = 7
}

variable "enable_container_insights" {
  description = "Enable ECS Container Insights"
  type        = bool
  default     = true
}

variable "ecr_repository_name" {
  description = "ECR repository name"
  type        = string
  default     = "facilitator"
}

variable "image_tag" {
  description = "Docker image tag"
  type        = string
  default     = "v1.3.6"
}
