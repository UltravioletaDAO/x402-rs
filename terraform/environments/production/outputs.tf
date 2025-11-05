# Terraform Outputs

output "vpc_id" {
  description = "VPC ID"
  value       = try(aws_vpc.main.id, "not-created-yet")
}

output "alb_dns_name" {
  description = "ALB DNS name"
  value       = try(aws_lb.main.dns_name, "not-created-yet")
}

output "alb_arn" {
  description = "ALB ARN"
  value       = try(aws_lb.main.arn, "not-created-yet")
}

output "ecs_cluster_name" {
  description = "ECS cluster name"
  value       = try(aws_ecs_cluster.main.name, "facilitator-production")
}

output "ecs_service_name" {
  description = "ECS service name"
  value       = try(aws_ecs_service.facilitator.name, "facilitator-production")
}

output "domain_name" {
  description = "Facilitator domain name"
  value       = var.domain_name
}

output "cloudwatch_log_group" {
  description = "CloudWatch log group name"
  value       = try(aws_cloudwatch_log_group.facilitator.name, "/ecs/facilitator-production")
}

output "ecr_repository_url" {
  description = "ECR repository URL"
  value       = "${data.aws_caller_identity.current.account_id}.dkr.ecr.${var.aws_region}.amazonaws.com/${var.ecr_repository_name}"
}
