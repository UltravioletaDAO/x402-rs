# ============================================================================
# Observability Stack - Grafana + Prometheus + Tempo
# ============================================================================
# Toggle with: enable_observability = true/false
#
# When enabled (~$10/month with Fargate Spot):
#   - OTel Collector sidecar in facilitator task (conditional in main.tf)
#   - Observability ECS task: Grafana + Prometheus + Tempo
#   - EFS for persistent storage
#   - Cloud Map for service discovery
#   - ALB host routing: metrics.facilitator.ultravioletadao.xyz -> Grafana
#
# When disabled ($0/month):
#   - Only ECR repos and ACM cert remain (both free, instant re-enable)
#   - No compute, no storage, no service discovery
#
# Architecture:
#   Facilitator Task: facilitator + otel-collector (sidecar)
#   Observability Task: grafana + prometheus + tempo
#   Communication: Cloud Map DNS (observability.facilitator.local)
# ============================================================================

# ----------------------------------------------------------------------------
# Variables
# ----------------------------------------------------------------------------

variable "enable_observability" {
  description = "Enable the observability stack (Grafana + Prometheus + Tempo). Set to false for $0/month."
  type        = bool
  default     = false
}

variable "metrics_domain_name" {
  description = "Domain name for Grafana metrics dashboard"
  type        = string
  default     = "metrics.facilitator.ultravioletadao.xyz"
}

variable "observability_task_cpu" {
  description = "CPU units for observability task (1024 = 1 vCPU)"
  type        = number
  default     = 1024
}

variable "observability_task_memory" {
  description = "Memory in MB for observability task"
  type        = number
  default     = 2048
}

variable "otel_collector_image_tag" {
  description = "Docker image tag for OTel Collector"
  type        = string
  default     = "latest"
}

variable "observability_image_tag" {
  description = "Docker image tag for observability stack (prometheus, tempo, grafana)"
  type        = string
  default     = "latest"
}

# ============================================================================
# ALWAYS ON (free resources - preserved for instant re-enable)
# ============================================================================

# ----------------------------------------------------------------------------
# ECR Repositories ($0 - images preserved across toggles)
# ----------------------------------------------------------------------------

resource "aws_ecr_repository" "otel_collector" {
  name                 = "facilitator-otel-collector"
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = false
  }

  tags = {
    Name = "facilitator-otel-collector"
  }
}

resource "aws_ecr_repository" "prometheus" {
  name                 = "facilitator-prometheus"
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = false
  }

  tags = {
    Name = "facilitator-prometheus"
  }
}

resource "aws_ecr_repository" "tempo" {
  name                 = "facilitator-tempo"
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = false
  }

  tags = {
    Name = "facilitator-tempo"
  }
}

resource "aws_ecr_repository" "grafana" {
  name                 = "facilitator-grafana"
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = false
  }

  tags = {
    Name = "facilitator-grafana"
  }
}

# ----------------------------------------------------------------------------
# ACM Certificate ($0 - avoids re-validation wait on re-enable)
# ----------------------------------------------------------------------------

resource "aws_acm_certificate" "metrics" {
  domain_name       = var.metrics_domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }

  tags = {
    Name = "metrics-${var.environment}"
  }
}

resource "aws_route53_record" "metrics_cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.metrics.domain_validation_options : dvo.domain_name => {
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

resource "aws_acm_certificate_validation" "metrics" {
  certificate_arn         = aws_acm_certificate.metrics.arn
  validation_record_fqdns = [for record in aws_route53_record.metrics_cert_validation : record.fqdn]
}

# Route53 A record ($0 - harmless pointer to ALB)
resource "aws_route53_record" "metrics" {
  zone_id = data.aws_route53_zone.main.zone_id
  name    = var.metrics_domain_name
  type    = "A"

  alias {
    name                   = aws_lb.main.dns_name
    zone_id                = aws_lb.main.zone_id
    evaluate_target_health = true
  }
}

# ============================================================================
# TOGGLED RESOURCES (only when enable_observability = true)
# ============================================================================

# ----------------------------------------------------------------------------
# Cloud Map Service Discovery
# ----------------------------------------------------------------------------

resource "aws_service_discovery_private_dns_namespace" "facilitator" {
  count       = var.enable_observability ? 1 : 0
  name        = "facilitator.local"
  description = "Service discovery for facilitator observability"
  vpc         = aws_vpc.main.id

  tags = {
    Name = "facilitator-${var.environment}-discovery"
  }
}

resource "aws_service_discovery_service" "observability" {
  count = var.enable_observability ? 1 : 0
  name  = "observability"

  dns_config {
    namespace_id = aws_service_discovery_private_dns_namespace.facilitator[0].id

    dns_records {
      ttl  = 10
      type = "A"
    }

    routing_policy = "MULTIVALUE"
  }

  health_check_custom_config {
    failure_threshold = 1
  }

  tags = {
    Name = "observability-${var.environment}"
  }
}

# ----------------------------------------------------------------------------
# EFS Filesystem (persistent storage for Prometheus + Tempo)
# ----------------------------------------------------------------------------

resource "aws_efs_file_system" "observability" {
  count          = var.enable_observability ? 1 : 0
  creation_token = "facilitator-observability-${var.environment}"
  encrypted      = true

  lifecycle_policy {
    transition_to_ia = "AFTER_14_DAYS"
  }

  tags = {
    Name = "facilitator-observability-${var.environment}"
  }
}

resource "aws_security_group" "efs" {
  count       = var.enable_observability ? 1 : 0
  name        = "facilitator-${var.environment}-efs"
  description = "Security group for EFS mount targets"
  vpc_id      = aws_vpc.main.id

  ingress {
    description     = "NFS from observability tasks"
    from_port       = 2049
    to_port         = 2049
    protocol        = "tcp"
    security_groups = [aws_security_group.observability[0].id]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "facilitator-${var.environment}-efs-sg"
  }
}

resource "aws_efs_mount_target" "observability" {
  count           = var.enable_observability ? length(aws_subnet.private) : 0
  file_system_id  = aws_efs_file_system.observability[0].id
  subnet_id       = aws_subnet.private[count.index].id
  security_groups = [aws_security_group.efs[0].id]
}

resource "aws_efs_access_point" "prometheus" {
  count          = var.enable_observability ? 1 : 0
  file_system_id = aws_efs_file_system.observability[0].id

  posix_user {
    gid = 65534
    uid = 65534
  }

  root_directory {
    path = "/prometheus"
    creation_info {
      owner_gid   = 65534
      owner_uid   = 65534
      permissions = "755"
    }
  }

  tags = {
    Name = "prometheus-${var.environment}"
  }
}

resource "aws_efs_access_point" "tempo" {
  count          = var.enable_observability ? 1 : 0
  file_system_id = aws_efs_file_system.observability[0].id

  posix_user {
    gid = 10001
    uid = 10001
  }

  root_directory {
    path = "/tempo"
    creation_info {
      owner_gid   = 10001
      owner_uid   = 10001
      permissions = "755"
    }
  }

  tags = {
    Name = "tempo-${var.environment}"
  }
}

resource "aws_efs_access_point" "grafana" {
  count          = var.enable_observability ? 1 : 0
  file_system_id = aws_efs_file_system.observability[0].id

  posix_user {
    gid = 472
    uid = 472
  }

  root_directory {
    path = "/grafana"
    creation_info {
      owner_gid   = 472
      owner_uid   = 472
      permissions = "755"
    }
  }

  tags = {
    Name = "grafana-${var.environment}"
  }
}

# ----------------------------------------------------------------------------
# Security Group for Observability Task
# ----------------------------------------------------------------------------

resource "aws_security_group" "observability" {
  count       = var.enable_observability ? 1 : 0
  name        = "facilitator-${var.environment}-observability"
  description = "Security group for observability ECS task"
  vpc_id      = aws_vpc.main.id

  ingress {
    description     = "Grafana from ALB"
    from_port       = 3000
    to_port         = 3000
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  ingress {
    description     = "Prometheus remote_write from facilitator"
    from_port       = 9090
    to_port         = 9090
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs_tasks.id]
  }

  ingress {
    description     = "Tempo OTLP gRPC from facilitator"
    from_port       = 4317
    to_port         = 4317
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs_tasks.id]
  }

  egress {
    description = "All outbound traffic"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "facilitator-${var.environment}-observability-sg"
  }
}

# ----------------------------------------------------------------------------
# CloudWatch Log Group for Observability
# ----------------------------------------------------------------------------

resource "aws_cloudwatch_log_group" "observability" {
  count             = var.enable_observability ? 1 : 0
  name              = "/ecs/observability-${var.environment}"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "observability-${var.environment}"
  }
}

# ----------------------------------------------------------------------------
# Grafana Admin Password Secret
# ----------------------------------------------------------------------------

data "aws_secretsmanager_secret" "grafana_admin_password" {
  count = var.enable_observability ? 1 : 0
  name  = "facilitator-grafana-admin-password"
}

# ----------------------------------------------------------------------------
# ALB: Listener Certificate + Host-Based Routing for Grafana
# ----------------------------------------------------------------------------

resource "aws_lb_listener_certificate" "metrics" {
  count           = var.enable_observability ? 1 : 0
  listener_arn    = aws_lb_listener.https.arn
  certificate_arn = aws_acm_certificate.metrics.arn

  depends_on = [aws_acm_certificate_validation.metrics]
}

resource "aws_lb_target_group" "grafana" {
  count       = var.enable_observability ? 1 : 0
  name        = "grafana-${var.environment}"
  port        = 3000
  protocol    = "HTTP"
  vpc_id      = aws_vpc.main.id
  target_type = "ip"

  health_check {
    enabled             = true
    healthy_threshold   = 2
    unhealthy_threshold = 3
    timeout             = 10
    interval            = 30
    path                = "/api/health"
    matcher             = "200"
  }

  deregistration_delay = 30

  tags = {
    Name = "grafana-${var.environment}-tg"
  }
}

resource "aws_lb_listener_rule" "metrics" {
  count        = var.enable_observability ? 1 : 0
  listener_arn = aws_lb_listener.https.arn
  priority     = 5

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.grafana[0].arn
  }

  condition {
    host_header {
      values = [var.metrics_domain_name]
    }
  }

  depends_on = [aws_lb_listener_certificate.metrics]
}

# ----------------------------------------------------------------------------
# IAM: Execution Role for Observability Task
# ----------------------------------------------------------------------------

resource "aws_iam_role" "observability_execution" {
  count = var.enable_observability ? 1 : 0
  name  = "observability-${var.environment}-ecs-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "ecs-tasks.amazonaws.com"
        }
      }
    ]
  })

  tags = {
    Name = "observability-${var.environment}-ecs-execution"
  }
}

resource "aws_iam_role_policy_attachment" "observability_execution" {
  count      = var.enable_observability ? 1 : 0
  role       = aws_iam_role.observability_execution[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "observability_secrets_access" {
  count = var.enable_observability ? 1 : 0
  name  = "secrets-access"
  role  = aws_iam_role.observability_execution[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = [
          data.aws_secretsmanager_secret.grafana_admin_password[0].arn
        ]
      }
    ]
  })
}

resource "aws_iam_role" "observability_task" {
  count = var.enable_observability ? 1 : 0
  name  = "observability-${var.environment}-ecs-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "ecs-tasks.amazonaws.com"
        }
      }
    ]
  })

  tags = {
    Name = "observability-${var.environment}-ecs-task"
  }
}

resource "aws_iam_role_policy" "observability_efs_access" {
  count = var.enable_observability ? 1 : 0
  name  = "efs-access"
  role  = aws_iam_role.observability_task[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "elasticfilesystem:ClientMount",
          "elasticfilesystem:ClientWrite",
          "elasticfilesystem:ClientRootAccess"
        ]
        Resource = aws_efs_file_system.observability[0].arn
      }
    ]
  })
}

# ----------------------------------------------------------------------------
# ECS Task Definition - Observability Stack
# ----------------------------------------------------------------------------

resource "aws_ecs_task_definition" "observability" {
  count                    = var.enable_observability ? 1 : 0
  family                   = "observability-${var.environment}"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.observability_task_cpu
  memory                   = var.observability_task_memory
  execution_role_arn       = aws_iam_role.observability_execution[0].arn
  task_role_arn            = aws_iam_role.observability_task[0].arn

  volume {
    name = "prometheus-data"

    efs_volume_configuration {
      file_system_id          = aws_efs_file_system.observability[0].id
      transit_encryption      = "ENABLED"
      authorization_config {
        access_point_id = aws_efs_access_point.prometheus[0].id
        iam             = "ENABLED"
      }
    }
  }

  volume {
    name = "tempo-data"

    efs_volume_configuration {
      file_system_id          = aws_efs_file_system.observability[0].id
      transit_encryption      = "ENABLED"
      authorization_config {
        access_point_id = aws_efs_access_point.tempo[0].id
        iam             = "ENABLED"
      }
    }
  }

  volume {
    name = "grafana-data"

    efs_volume_configuration {
      file_system_id          = aws_efs_file_system.observability[0].id
      transit_encryption      = "ENABLED"
      authorization_config {
        access_point_id = aws_efs_access_point.grafana[0].id
        iam             = "ENABLED"
      }
    }
  }

  container_definitions = jsonencode([
    # ----- Grafana -----
    {
      name      = "grafana"
      image     = "${aws_ecr_repository.grafana.repository_url}:${var.observability_image_tag}"
      essential = true

      portMappings = [
        {
          containerPort = 3000
          protocol      = "tcp"
        }
      ]

      environment = [
        {
          name  = "GF_SECURITY_ADMIN_USER"
          value = "admin"
        }
      ]

      secrets = [
        {
          name      = "GF_SECURITY_ADMIN_PASSWORD"
          valueFrom = data.aws_secretsmanager_secret.grafana_admin_password[0].arn
        }
      ]

      mountPoints = [
        {
          sourceVolume  = "grafana-data"
          containerPath = "/var/lib/grafana"
          readOnly      = false
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.observability[0].name
          "awslogs-region"        = data.aws_region.current.name
          "awslogs-stream-prefix" = "grafana"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "wget --spider -q http://localhost:3000/api/health || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 30
      }
    },

    # ----- Prometheus -----
    {
      name      = "prometheus"
      image     = "${aws_ecr_repository.prometheus.repository_url}:${var.observability_image_tag}"
      essential = true

      portMappings = [
        {
          containerPort = 9090
          protocol      = "tcp"
        }
      ]

      command = [
        "--config.file=/etc/prometheus/prometheus.yml",
        "--storage.tsdb.path=/prometheus",
        "--storage.tsdb.retention.time=15d",
        "--web.enable-remote-write-receiver",
        "--enable-feature=exemplar-storage"
      ]

      mountPoints = [
        {
          sourceVolume  = "prometheus-data"
          containerPath = "/prometheus"
          readOnly      = false
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.observability[0].name
          "awslogs-region"        = data.aws_region.current.name
          "awslogs-stream-prefix" = "prometheus"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "wget --spider -q http://localhost:9090/-/healthy || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 30
      }
    },

    # ----- Tempo -----
    {
      name      = "tempo"
      image     = "${aws_ecr_repository.tempo.repository_url}:${var.observability_image_tag}"
      essential = true

      portMappings = [
        {
          containerPort = 3200
          protocol      = "tcp"
        },
        {
          containerPort = 4317
          protocol      = "tcp"
        }
      ]

      command = ["-config.file=/etc/tempo/tempo.yml"]

      mountPoints = [
        {
          sourceVolume  = "tempo-data"
          containerPath = "/var/tempo"
          readOnly      = false
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.observability[0].name
          "awslogs-region"        = data.aws_region.current.name
          "awslogs-stream-prefix" = "tempo"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "wget --spider -q http://localhost:3200/ready || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 30
      }
    }
  ])

  tags = {
    Name = "observability-${var.environment}"
  }
}

# ----------------------------------------------------------------------------
# ECS Service - Observability (Fargate Spot for ~70% cost savings)
# ----------------------------------------------------------------------------

resource "aws_ecs_service" "observability" {
  count           = var.enable_observability ? 1 : 0
  name            = "observability-${var.environment}"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.observability[0].arn
  desired_count   = 1

  # Fargate Spot: ~70% cheaper, acceptable for monitoring workload
  capacity_provider_strategy {
    capacity_provider = "FARGATE_SPOT"
    weight            = 1
    base              = 1
  }

  # Platform version 1.4.0+ required for EFS
  platform_version = "1.4.0"

  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.observability[0].id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.grafana[0].arn
    container_name   = "grafana"
    container_port   = 3000
  }

  service_registries {
    registry_arn = aws_service_discovery_service.observability[0].arn
  }

  depends_on = [
    aws_lb_listener_rule.metrics,
    aws_efs_mount_target.observability
  ]

  tags = {
    Name = "observability-${var.environment}"
  }
}

# ----------------------------------------------------------------------------
# Outputs
# ----------------------------------------------------------------------------

output "metrics_domain" {
  description = "Grafana metrics dashboard URL"
  value       = "https://${var.metrics_domain_name}"
}

output "observability_enabled" {
  description = "Whether the observability stack is deployed"
  value       = var.enable_observability
}

output "observability_service_discovery" {
  description = "Cloud Map DNS name for observability service"
  value       = var.enable_observability ? "observability.facilitator.local" : "disabled"
}

output "otel_collector_ecr_url" {
  description = "ECR URL for OTel Collector image"
  value       = aws_ecr_repository.otel_collector.repository_url
}

output "prometheus_ecr_url" {
  description = "ECR URL for Prometheus image"
  value       = aws_ecr_repository.prometheus.repository_url
}

output "tempo_ecr_url" {
  description = "ECR URL for Tempo image"
  value       = aws_ecr_repository.tempo.repository_url
}

output "grafana_ecr_url" {
  description = "ECR URL for Grafana image"
  value       = aws_ecr_repository.grafana.repository_url
}
