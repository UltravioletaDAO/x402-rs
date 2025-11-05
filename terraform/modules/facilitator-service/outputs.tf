# ============================================================================
# OUTPUTS - Important Values After Deployment
# ============================================================================

# ----------------------------------------------------------------------------
# VPC Outputs
# ----------------------------------------------------------------------------

output "vpc_id" {
  description = "VPC ID"
  value       = aws_vpc.main.id
}

output "vpc_cidr" {
  description = "VPC CIDR block"
  value       = aws_vpc.main.cidr_block
}

output "public_subnet_ids" {
  description = "Public subnet IDs"
  value       = aws_subnet.public[*].id
}

output "private_subnet_ids" {
  description = "Private subnet IDs"
  value       = aws_subnet.private[*].id
}

output "nat_gateway_ids" {
  description = "NAT Gateway IDs"
  value       = aws_nat_gateway.main[*].id
}

output "nat_gateway_public_ips" {
  description = "NAT Gateway Elastic IPs (for whitelisting)"
  value       = aws_eip.nat[*].public_ip
}

# ----------------------------------------------------------------------------
# ECS Outputs
# ----------------------------------------------------------------------------

output "ecs_cluster_id" {
  description = "ECS Cluster ID"
  value       = aws_ecs_cluster.main.id
}

output "ecs_cluster_name" {
  description = "ECS Cluster name"
  value       = aws_ecs_cluster.main.name
}

output "ecs_cluster_arn" {
  description = "ECS Cluster ARN"
  value       = aws_ecs_cluster.main.arn
}

output "ecs_service_names" {
  description = "ECS Service names"
  value       = { for k, v in aws_ecs_service.agents : k => v.name }
}

output "ecs_service_arns" {
  description = "ECS Service ARNs"
  value       = { for k, v in aws_ecs_service.agents : k => v.id }
}

output "ecs_task_definition_arns" {
  description = "ECS Task Definition ARNs"
  value       = { for k, v in aws_ecs_task_definition.agents : k => v.arn }
}

# ----------------------------------------------------------------------------
# Load Balancer Outputs
# ----------------------------------------------------------------------------

output "alb_id" {
  description = "Application Load Balancer ID"
  value       = aws_lb.main.id
}

output "alb_arn" {
  description = "Application Load Balancer ARN"
  value       = aws_lb.main.arn
}

output "alb_dns_name" {
  description = "Application Load Balancer DNS name (use this to access agents)"
  value       = aws_lb.main.dns_name
}

output "alb_zone_id" {
  description = "Application Load Balancer Route53 Zone ID"
  value       = aws_lb.main.zone_id
}

output "target_group_arns" {
  description = "Target Group ARNs"
  value       = { for k, v in aws_lb_target_group.agents : k => v.arn }
}

# ----------------------------------------------------------------------------
# Agent Endpoints
# ----------------------------------------------------------------------------

output "agent_endpoints" {
  description = "Agent HTTP endpoints (via ALB)"
  value = {
    for k, v in var.agents : k => "http://${aws_lb.main.dns_name}/${k}"
  }
}

output "agent_health_check_urls" {
  description = "Agent health check URLs (path-based)"
  value = {
    for k, v in var.agents : k => "http://${aws_lb.main.dns_name}/${k}${v.health_check_path}"
  }
}

# ----------------------------------------------------------------------------
# Route53 Domain Outputs
# ----------------------------------------------------------------------------

output "base_domain" {
  description = "Base domain for karmacadabra"
  value       = var.enable_route53 ? var.base_domain : "Route53 disabled"
}

output "agent_domains" {
  description = "Agent domain names (hostname-based routing)"
  value = var.enable_route53 ? {
    for k, v in var.agents : k => "${k}.${var.base_domain}"
  } : {}
}

output "agent_domain_endpoints" {
  description = "Agent HTTP endpoints using custom domains"
  value = var.enable_route53 ? {
    for k, v in var.agents : k => "http://${k}.${var.base_domain}"
  } : {}
}

output "agent_domain_health_check_urls" {
  description = "Agent health check URLs using custom domains"
  value = var.enable_route53 ? {
    for k, v in var.agents : k => "${var.enable_https ? "https" : "http"}://${k}.${var.base_domain}${v.health_check_path}"
  } : {}
}

# ----------------------------------------------------------------------------
# HTTPS/SSL Outputs
# ----------------------------------------------------------------------------

output "acm_certificate_arn" {
  description = "ACM certificate ARN"
  value       = var.enable_https ? aws_acm_certificate.main[0].arn : null
}

output "acm_certificate_status" {
  description = "ACM certificate validation status"
  value       = var.enable_https ? aws_acm_certificate.main[0].status : null
}

output "https_enabled" {
  description = "Whether HTTPS is enabled"
  value       = var.enable_https
}

# ----------------------------------------------------------------------------
# ECR Outputs
# ----------------------------------------------------------------------------

output "ecr_repository_urls" {
  description = "ECR repository URLs (use these to push Docker images)"
  value       = { for k, v in aws_ecr_repository.agents : k => v.repository_url }
}

output "ecr_repository_arns" {
  description = "ECR repository ARNs"
  value       = { for k, v in aws_ecr_repository.agents : k => v.arn }
}

# ----------------------------------------------------------------------------
# IAM Outputs
# ----------------------------------------------------------------------------

output "ecs_task_execution_role_arn" {
  description = "ECS Task Execution Role ARN"
  value       = aws_iam_role.ecs_task_execution.arn
}

output "ecs_task_role_arn" {
  description = "ECS Task Role ARN"
  value       = aws_iam_role.ecs_task.arn
}

# ----------------------------------------------------------------------------
# CloudWatch Outputs
# ----------------------------------------------------------------------------

output "cloudwatch_log_group_names" {
  description = "CloudWatch Log Group names"
  value       = { for k, v in aws_cloudwatch_log_group.agents : k => v.name }
}

# Dashboard disabled - see cloudwatch.tf
# output "cloudwatch_dashboard_name" {
#   description = "CloudWatch Dashboard name"
#   value       = aws_cloudwatch_dashboard.main.dashboard_name
# }

# output "cloudwatch_dashboard_url" {
#   description = "CloudWatch Dashboard URL"
#   value       = "https://console.aws.amazon.com/cloudwatch/home?region=${var.aws_region}#dashboards:name=${aws_cloudwatch_dashboard.main.dashboard_name}"
# }

# ----------------------------------------------------------------------------
# Security Group Outputs
# ----------------------------------------------------------------------------

output "alb_security_group_id" {
  description = "ALB Security Group ID"
  value       = aws_security_group.alb.id
}

output "ecs_tasks_security_group_id" {
  description = "ECS Tasks Security Group ID"
  value       = aws_security_group.ecs_tasks.id
}

# ----------------------------------------------------------------------------
# Service Discovery Outputs
# ----------------------------------------------------------------------------

output "service_discovery_namespace" {
  description = "Service Discovery namespace (for inter-agent communication)"
  value       = var.enable_service_connect ? aws_service_discovery_private_dns_namespace.main[0].name : null
}

output "service_connect_endpoints" {
  description = "Service Connect DNS names (for inter-agent communication)"
  value = var.enable_service_connect ? {
    for k, v in var.agents : k => "${k}.${var.service_connect_namespace}:${v.port}"
  } : null
}

# ----------------------------------------------------------------------------
# Cost Estimation
# ----------------------------------------------------------------------------

output "estimated_monthly_cost_usd" {
  description = "Estimated monthly cost in USD (approximate)"
  value = {
    fargate_tasks = format("~$%s (5 agents, 24/7, %s)",
      var.use_fargate_spot ? "25-40" : "80-120",
      var.use_fargate_spot ? "Spot" : "On-Demand")
    alb           = "~$16-18"
    nat_gateway   = var.enable_nat_gateway ? (var.single_nat_gateway ? "~$32" : "~$64") : "$0"
    cloudwatch    = "~$5-8 (logs + metrics)"
    ecr           = "~$1-2 (image storage)"
    total         = format("~$%s", var.use_fargate_spot ? "79-96" : "134-212")
    notes         = "Costs can be reduced by: 1) Scaling down to 0 tasks when not in use, 2) Using smaller task sizes, 3) Reducing log retention"
  }
}

# ----------------------------------------------------------------------------
# Deployment Commands
# ----------------------------------------------------------------------------

output "deployment_commands" {
  description = "Useful commands for deployment and management"
  value = {
    ecr_login = "aws ecr get-login-password --region ${var.aws_region} | docker login --username AWS --password-stdin ${data.aws_caller_identity.current.account_id}.dkr.ecr.${var.aws_region}.amazonaws.com"

    build_and_push_example = "docker build -t karmacadabra/validator . && docker tag karmacadabra/validator:latest ${aws_ecr_repository.agents["validator"].repository_url}:latest && docker push ${aws_ecr_repository.agents["validator"].repository_url}:latest"

    force_new_deployment = "aws ecs update-service --cluster ${aws_ecs_cluster.main.name} --service ${var.project_name}-${var.environment}-validator --force-new-deployment"

    view_logs = "aws logs tail /ecs/${var.project_name}-${var.environment}/validator --follow"

    ecs_exec_into_task = "aws ecs execute-command --cluster ${aws_ecs_cluster.main.name} --task TASK_ID --container validator --interactive --command '/bin/bash'"

    describe_service = "aws ecs describe-services --cluster ${aws_ecs_cluster.main.name} --services ${var.project_name}-${var.environment}-validator"
  }
}

# ----------------------------------------------------------------------------
# Quick Start Guide
# ----------------------------------------------------------------------------

output "quick_start" {
  description = "Quick start guide for using the infrastructure"
  value = <<-EOT

    KARMACADABRA ECS DEPLOYMENT SUCCESSFUL!
    =======================================

    1. BUILD AND PUSH DOCKER IMAGES:
       cd /home/user/karmacadabra

       # Login to ECR
       ${replace(
         "aws ecr get-login-password --region ${var.aws_region} | docker login --username AWS --password-stdin ${data.aws_caller_identity.current.account_id}.dkr.ecr.${var.aws_region}.amazonaws.com",
         "\n", "\n       "
       )}

       # Build and push each agent
       docker build -f Dockerfile.agent -t karmacadabra/validator .
       docker tag karmacadabra/validator:latest ${aws_ecr_repository.agents["validator"].repository_url}:latest
       docker push ${aws_ecr_repository.agents["validator"].repository_url}:latest

       # Repeat for other agents: karma-hello, abracadabra, skill-extractor, voice-extractor

    2. ACCESS AGENTS:
       ${var.enable_route53 ? "CUSTOM DOMAINS (Recommended):" : "ALB DNS:"}
       ${var.enable_route53 ? "" : aws_lb.main.dns_name}

       ${var.enable_route53 ? "Validator:       http://validator.${var.base_domain}/health" : "Validator:       http://${aws_lb.main.dns_name}/validator/health"}
       ${var.enable_route53 ? "Karma-Hello:     http://karma-hello.${var.base_domain}/health" : "Karma-Hello:     http://${aws_lb.main.dns_name}/karma-hello/health"}
       ${var.enable_route53 ? "Abracadabra:     http://abracadabra.${var.base_domain}/health" : "Abracadabra:     http://${aws_lb.main.dns_name}/abracadabra/health"}
       ${var.enable_route53 ? "Skill-Extractor: http://skill-extractor.${var.base_domain}/health" : "Skill-Extractor: http://${aws_lb.main.dns_name}/skill-extractor/health"}
       ${var.enable_route53 ? "Voice-Extractor: http://voice-extractor.${var.base_domain}/health" : "Voice-Extractor: http://${aws_lb.main.dns_name}/voice-extractor/health"}

       ${var.enable_route53 ? "PATH-BASED ROUTING (Also Available):" : ""}
       ${var.enable_route53 ? "http://${aws_lb.main.dns_name}/validator/health" : ""}
       ${var.enable_route53 ? "http://${aws_lb.main.dns_name}/karma-hello/health" : ""}
       (etc.)

    3. VIEW LOGS:
       aws logs tail /ecs/${var.project_name}-${var.environment}/validator --follow

    4. MONITOR:
       CloudWatch Logs: View logs for each agent in CloudWatch console
       CloudWatch Alarms: Monitor CPU/Memory/Task count alarms
       (Dashboard disabled - see cloudwatch.tf for details)

    5. DEBUG (SSH into container):
       # Get task ID
       TASK_ID=$(aws ecs list-tasks --cluster ${aws_ecs_cluster.main.name} --service-name ${var.project_name}-${var.environment}-validator --query 'taskArns[0]' --output text | cut -d'/' -f3)

       # Execute command
       aws ecs execute-command --cluster ${aws_ecs_cluster.main.name} --task $TASK_ID --container validator --interactive --command '/bin/bash'

    ESTIMATED MONTHLY COST: $${var.use_fargate_spot ? "79-96" : "134-212"}
    - Using ${var.use_fargate_spot ? "Fargate SPOT (70% savings)" : "Fargate On-Demand"}
    - ${var.single_nat_gateway ? "Single NAT Gateway (cost optimized)" : "Multi-AZ NAT Gateways"}
    - Container Insights: ${var.enable_container_insights ? "Enabled" : "Disabled"}
  EOT
}
