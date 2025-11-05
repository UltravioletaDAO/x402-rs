# ============================================================================
# VARIABLES - Karmacadabra ECS Fargate Infrastructure
# ============================================================================
# Cost-optimized configuration with sensible defaults for AI agent deployment

# ----------------------------------------------------------------------------
# General Configuration
# ----------------------------------------------------------------------------

variable "project_name" {
  description = "Project name used for resource naming and tagging"
  type        = string
  default     = "karmacadabra"
}

variable "environment" {
  description = "Environment name (dev, staging, prod)"
  type        = string
  default     = "prod"
}

variable "aws_region" {
  description = "AWS region for deployment"
  type        = string
  default     = "us-east-1" # Cheapest region for most services
}

variable "tags" {
  description = "Common tags applied to all resources"
  type        = map(string)
  default = {
    Project     = "Karmacadabra"
    ManagedBy   = "Terraform"
    Environment = "prod"
  }
}

# ----------------------------------------------------------------------------
# VPC Configuration
# ----------------------------------------------------------------------------

variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.0.0.0/16"
}

variable "availability_zones" {
  description = "Availability zones for deployment (COST: Use 2 AZs for ALB, but 1 NAT)"
  type        = list(string)
  default     = ["us-east-1a", "us-east-1b"]
}

variable "enable_nat_gateway" {
  description = "Enable NAT Gateway for private subnet internet access (COST: $32/month)"
  type        = bool
  default     = true
}

variable "single_nat_gateway" {
  description = "Use single NAT Gateway instead of one per AZ (COST SAVINGS: ~$32/month)"
  type        = bool
  default     = true # CRITICAL: Single NAT saves ~50% on NAT costs
}

variable "enable_vpc_endpoints" {
  description = "Enable VPC endpoints for AWS services (COST: Reduces NAT data transfer)"
  type        = bool
  default     = true
}

# ----------------------------------------------------------------------------
# ECS Configuration
# ----------------------------------------------------------------------------

variable "enable_container_insights" {
  description = "Enable CloudWatch Container Insights (COST: ~$3-5/month for metrics)"
  type        = bool
  default     = true
}

variable "enable_execute_command" {
  description = "Enable ECS Exec for debugging (ssh into containers)"
  type        = bool
  default     = true
}

# ----------------------------------------------------------------------------
# Fargate Task Configuration (COST CRITICAL)
# ----------------------------------------------------------------------------

variable "task_cpu" {
  description = "Fargate task CPU units (256 = 0.25 vCPU) - COST: Start smallest possible"
  type        = number
  default     = 256 # 0.25 vCPU - smallest Fargate size
}

variable "task_memory" {
  description = "Fargate task memory in MB - COST: Start smallest possible"
  type        = number
  default     = 512 # 0.5 GB - smallest for 256 CPU
}

# Facilitator-specific configuration (higher specs needed)
variable "facilitator_task_cpu" {
  description = "Fargate task CPU units for facilitator (2048 = 2 vCPU)"
  type        = number
  default     = 2048 # 2 vCPU for facilitator
}

variable "facilitator_task_memory" {
  description = "Fargate task memory in MB for facilitator"
  type        = number
  default     = 4096 # 4 GB for facilitator
}

variable "desired_count_per_service" {
  description = "Number of tasks to run per service - COST: Start with 1"
  type        = number
  default     = 1
}

variable "use_fargate_spot" {
  description = "Use Fargate Spot for 70% cost savings (CRITICAL FOR COST)"
  type        = bool
  default     = true # MUST BE TRUE - saves ~$30/month per service
}

variable "fargate_spot_base_capacity" {
  description = "Base capacity on on-demand Fargate (0-100%)"
  type        = number
  default     = 0 # 100% Spot for maximum savings
}

variable "fargate_spot_weight" {
  description = "Weight for Spot capacity (higher = prefer Spot)"
  type        = number
  default     = 100
}

variable "fargate_ondemand_weight" {
  description = "Weight for On-Demand capacity (higher = prefer On-Demand)"
  type        = number
  default     = 1
}

# ----------------------------------------------------------------------------
# Auto-Scaling Configuration (COST OPTIMIZATION)
# ----------------------------------------------------------------------------

variable "enable_autoscaling" {
  description = "Enable auto-scaling for services"
  type        = bool
  default     = true
}

variable "autoscaling_min_capacity" {
  description = "Minimum number of tasks"
  type        = number
  default     = 1
}

variable "autoscaling_max_capacity" {
  description = "Maximum number of tasks - COST: Keep conservative"
  type        = number
  default     = 3 # Max 3 tasks per service to control costs
}

variable "autoscaling_cpu_target" {
  description = "Target CPU utilization % for scaling"
  type        = number
  default     = 75 # Scale up at 75% CPU
}

variable "autoscaling_memory_target" {
  description = "Target memory utilization % for scaling"
  type        = number
  default     = 80 # Scale up at 80% memory
}

# ----------------------------------------------------------------------------
# Agent Configuration
# ----------------------------------------------------------------------------

variable "agents" {
  description = "Map of agent configurations"
  type = map(object({
    port              = number
    health_check_path = string
    priority          = number # ALB listener rule priority
  }))
  default = {
    facilitator = {
      port              = 8080
      health_check_path = "/health"
      priority          = 50
    }
    validator = {
      port              = 9001
      health_check_path = "/health"
      priority          = 100
    }
    karma-hello = {
      port              = 9002
      health_check_path = "/health"
      priority          = 200
    }
    abracadabra = {
      port              = 9003
      health_check_path = "/health"
      priority          = 300
    }
    skill-extractor = {
      port              = 9004
      health_check_path = "/health"
      priority          = 400
    }
    voice-extractor = {
      port              = 9005
      health_check_path = "/health"
      priority          = 500
    }
    marketplace = {
      port              = 9000
      health_check_path = "/health"
      priority          = 600
    }
    test-seller = {
      port              = 8080
      health_check_path = "/health"
      priority          = 700
    }
  }
}

# ----------------------------------------------------------------------------
# Secrets Configuration
# ----------------------------------------------------------------------------

variable "secrets_manager_secret_name" {
  description = "AWS Secrets Manager secret name containing agent credentials"
  type        = string
  default     = "karmacadabra"
}

# ----------------------------------------------------------------------------
# Load Balancer Configuration
# ----------------------------------------------------------------------------

variable "alb_idle_timeout" {
  description = "ALB idle timeout in seconds (CRITICAL: Must be > Base mainnet settlement time)"
  type        = number
  default     = 180  # Increased from 60s to accommodate Base mainnet tx confirmations
}

variable "enable_alb_access_logs" {
  description = "Enable ALB access logs to S3 (COST: Adds S3 storage costs)"
  type        = bool
  default     = false # Disabled to save costs
}

variable "alb_deletion_protection" {
  description = "Enable ALB deletion protection"
  type        = bool
  default     = false # Disabled for easier testing
}

# ----------------------------------------------------------------------------
# CloudWatch Configuration (COST OPTIMIZATION)
# ----------------------------------------------------------------------------

variable "log_retention_days" {
  description = "CloudWatch Logs retention in days (COST: Shorter = cheaper)"
  type        = number
  default     = 7 # 7 days to minimize storage costs
}

variable "enable_xray_tracing" {
  description = "Enable AWS X-Ray tracing (COST: ~$5/month for 100K traces)"
  type        = bool
  default     = true
}

# ----------------------------------------------------------------------------
# Health Check Configuration
# ----------------------------------------------------------------------------

variable "health_check_interval" {
  description = "Health check interval in seconds"
  type        = number
  default     = 30
}

variable "health_check_timeout" {
  description = "Health check timeout in seconds"
  type        = number
  default     = 5
}

variable "health_check_healthy_threshold" {
  description = "Number of consecutive successful health checks"
  type        = number
  default     = 2
}

variable "health_check_unhealthy_threshold" {
  description = "Number of consecutive failed health checks"
  type        = number
  default     = 3
}

# ----------------------------------------------------------------------------
# Service Connect Configuration (COST SAVINGS)
# ----------------------------------------------------------------------------

variable "enable_service_connect" {
  description = "Enable ECS Service Connect for inter-agent communication (COST: No ALB needed)"
  type        = bool
  default     = true # Enables agents to call each other without ALB
}

variable "service_connect_namespace" {
  description = "CloudMap namespace for Service Connect"
  type        = string
  default     = "karmacadabra.local"
}

# ----------------------------------------------------------------------------
# Route53 Domain Configuration
# ----------------------------------------------------------------------------

variable "enable_route53" {
  description = "Enable Route53 DNS record creation"
  type        = bool
  default     = true
}

variable "hosted_zone_name" {
  description = "Route53 hosted zone name (e.g., ultravioletadao.xyz)"
  type        = string
  default     = "ultravioletadao.xyz"
}

variable "base_domain" {
  description = "Base domain for karmacadabra (under hosted zone)"
  type        = string
  default     = "karmacadabra.ultravioletadao.xyz"
}

variable "enable_wildcard_domain" {
  description = "Create wildcard DNS record (*.karmacadabra.ultravioletadao.xyz)"
  type        = bool
  default     = false
}

variable "enable_hostname_routing" {
  description = "Enable hostname-based routing in ALB (in addition to path-based)"
  type        = bool
  default     = true
}

# ----------------------------------------------------------------------------
# HTTPS/SSL Configuration
# ----------------------------------------------------------------------------

variable "enable_https" {
  description = "Enable HTTPS with ACM certificate"
  type        = bool
  default     = true
}

variable "ssl_policy" {
  description = "SSL policy for HTTPS listener"
  type        = string
  default     = "ELBSecurityPolicy-TLS-1-2-2017-01"
}

variable "redirect_http_to_https" {
  description = "Redirect HTTP traffic to HTTPS"
  type        = bool
  default     = true
}

# ----------------------------------------------------------------------------
# Monitoring & Alerting
# ----------------------------------------------------------------------------

variable "alarm_sns_topic_name" {
  description = "SNS topic name for CloudWatch alarms (empty = no SNS)"
  type        = string
  default     = "" # Empty = no SNS notifications (saves costs)
}

variable "enable_high_cpu_alarm" {
  description = "Enable high CPU utilization alarm"
  type        = bool
  default     = true
}

variable "enable_high_memory_alarm" {
  description = "Enable high memory utilization alarm"
  type        = bool
  default     = true
}

variable "enable_task_count_alarm" {
  description = "Enable low task count alarm"
  type        = bool
  default     = true
}

variable "cpu_alarm_threshold" {
  description = "CPU utilization threshold for alarm (%)"
  type        = number
  default     = 85
}

variable "memory_alarm_threshold" {
  description = "Memory utilization threshold for alarm (%)"
  type        = number
  default     = 85
}
