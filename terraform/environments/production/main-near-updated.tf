# ============================================================================
# Facilitator Production Infrastructure - us-east-2
# ============================================================================
# Standalone deployment for facilitator.ultravioletadao.xyz
# Cost-optimized configuration: ~$43-48/month
# UPDATED: NEAR Protocol support added

# Data Sources
data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

# Route53 Hosted Zone (must already exist)
data "aws_route53_zone" "main" {
  name         = var.hosted_zone_name
  private_zone = false
}

# Secrets for facilitator
data "aws_secretsmanager_secret" "evm_private_key" {
  name = var.evm_secret_name
}

data "aws_secretsmanager_secret" "solana_keypair" {
  name = var.solana_secret_name
}

# RPC endpoints secret (QuickNode mainnet endpoints)
data "aws_secretsmanager_secret" "rpc_mainnet" {
  name = "facilitator-rpc-mainnet"
}

# RPC endpoints secret (testnet endpoints)
data "aws_secretsmanager_secret" "rpc_testnet" {
  name = "facilitator-rpc-testnet"
}

# NEAR keypair secrets (ADDED FOR NEAR SUPPORT)
data "aws_secretsmanager_secret" "near_mainnet_keypair" {
  name = "facilitator-near-mainnet-keypair"
}

data "aws_secretsmanager_secret" "near_testnet_keypair" {
  name = "facilitator-near-testnet-keypair"
}

# ============================================================================
# VPC and Networking
# ============================================================================

resource "aws_vpc" "main" {
  cidr_block           = var.vpc_cidr
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = {
    Name = "facilitator-${var.environment}"
  }
}

# Internet Gateway
resource "aws_internet_gateway" "main" {
  vpc_id = aws_vpc.main.id

  tags = {
    Name = "facilitator-${var.environment}-igw"
  }
}

# Public Subnets (for ALB)
resource "aws_subnet" "public" {
  count                   = length(var.availability_zones)
  vpc_id                  = aws_vpc.main.id
  cidr_block              = cidrsubnet(var.vpc_cidr, 8, count.index)
  availability_zone       = var.availability_zones[count.index]
  map_public_ip_on_launch = true

  tags = {
    Name = "facilitator-${var.environment}-public-${var.availability_zones[count.index]}"
  }
}

# Private Subnets (for ECS tasks)
resource "aws_subnet" "private" {
  count             = length(var.availability_zones)
  vpc_id            = aws_vpc.main.id
  cidr_block        = cidrsubnet(var.vpc_cidr, 8, count.index + 100)
  availability_zone = var.availability_zones[count.index]

  tags = {
    Name = "facilitator-${var.environment}-private-${var.availability_zones[count.index]}"
  }
}

# Elastic IP for NAT
resource "aws_eip" "nat" {
  domain = "vpc"

  tags = {
    Name = "facilitator-${var.environment}-nat-eip"
  }
}

# NAT Gateway (for private subnets to reach internet)
resource "aws_nat_gateway" "main" {
  allocation_id = aws_eip.nat.id
  subnet_id     = aws_subnet.public[0].id

  tags = {
    Name = "facilitator-${var.environment}-nat"
  }

  depends_on = [aws_internet_gateway.main]
}

# Route Table for Public Subnets
resource "aws_route_table" "public" {
  vpc_id = aws_vpc.main.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.main.id
  }

  tags = {
    Name = "facilitator-${var.environment}-public-rt"
  }
}

# Route Table for Private Subnets
resource "aws_route_table" "private" {
  vpc_id = aws_vpc.main.id

  route {
    cidr_block     = "0.0.0.0/0"
    nat_gateway_id = aws_nat_gateway.main.id
  }

  tags = {
    Name = "facilitator-${var.environment}-private-rt"
  }
}

# Associate Public Subnets with Public Route Table
resource "aws_route_table_association" "public" {
  count          = length(aws_subnet.public)
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# Associate Private Subnets with Private Route Table
resource "aws_route_table_association" "private" {
  count          = length(aws_subnet.private)
  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private.id
}

# ============================================================================
# Security Groups
# ============================================================================

# ALB Security Group
resource "aws_security_group" "alb" {
  name        = "facilitator-${var.environment}-alb"
  description = "Security group for facilitator ALB"
  vpc_id      = aws_vpc.main.id

  ingress {
    description = "HTTPS from internet"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "HTTP from internet (redirect to HTTPS)"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "All outbound traffic"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "facilitator-${var.environment}-alb-sg"
  }
}

# ECS Tasks Security Group
resource "aws_security_group" "ecs_tasks" {
  name        = "facilitator-${var.environment}-ecs-tasks"
  description = "Security group for facilitator ECS tasks"
  vpc_id      = aws_vpc.main.id

  ingress {
    description     = "Traffic from ALB"
    from_port       = 8080
    to_port         = 8080
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  egress {
    description = "All outbound traffic (for RPC calls)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "facilitator-${var.environment}-ecs-tasks-sg"
  }
}

# ============================================================================
# ACM Certificate
# ============================================================================

resource "aws_acm_certificate" "main" {
  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }

  tags = {
    Name = "facilitator-${var.environment}"
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

# ============================================================================
# Application Load Balancer
# ============================================================================

resource "aws_lb" "main" {
  name               = "facilitator-${var.environment}"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb.id]
  subnets            = aws_subnet.public[*].id

  enable_deletion_protection = false
  enable_http2              = true
  idle_timeout              = var.alb_idle_timeout

  tags = {
    Name = "facilitator-${var.environment}-alb"
  }
}

# Target Group
resource "aws_lb_target_group" "main" {
  name        = "facilitator-${var.environment}"
  port        = 8080
  protocol    = "HTTP"
  vpc_id      = aws_vpc.main.id
  target_type = "ip"

  health_check {
    enabled             = true
    healthy_threshold   = 2
    unhealthy_threshold = 3
    timeout             = 30
    interval            = 60
    path                = "/health"
    matcher             = "200"
  }

  deregistration_delay = 30

  tags = {
    Name = "facilitator-${var.environment}-tg"
  }
}

# HTTPS Listener
resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.main.arn
  port              = "443"
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate.main.arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.main.arn
  }

  depends_on = [aws_acm_certificate_validation.main]
}

# HTTP Listener (redirect to HTTPS)
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.main.arn
  port              = "80"
  protocol          = "HTTP"

  default_action {
    type = "redirect"

    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

# ============================================================================
# Route53 DNS
# ============================================================================

resource "aws_route53_record" "main" {
  zone_id = data.aws_route53_zone.main.zone_id
  name    = var.domain_name
  type    = "A"

  alias {
    name                   = aws_lb.main.dns_name
    zone_id                = aws_lb.main.zone_id
    evaluate_target_health = true
  }
}

# ============================================================================
# CloudWatch Log Group
# ============================================================================

resource "aws_cloudwatch_log_group" "facilitator" {
  name              = "/ecs/facilitator-${var.environment}"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "facilitator-${var.environment}"
  }
}

# ============================================================================
# IAM Roles
# ============================================================================

# ECS Task Execution Role
resource "aws_iam_role" "ecs_task_execution" {
  name = "facilitator-${var.environment}-ecs-execution"

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
    Name = "facilitator-${var.environment}-ecs-execution"
  }
}

resource "aws_iam_role_policy_attachment" "ecs_task_execution" {
  role       = aws_iam_role.ecs_task_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# Policy for accessing secrets (UPDATED WITH NEAR SECRETS)
resource "aws_iam_role_policy" "secrets_access" {
  name = "secrets-access"
  role = aws_iam_role.ecs_task_execution.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = [
          data.aws_secretsmanager_secret.evm_private_key.arn,
          data.aws_secretsmanager_secret.solana_keypair.arn,
          data.aws_secretsmanager_secret.rpc_mainnet.arn,
          data.aws_secretsmanager_secret.rpc_testnet.arn,
          data.aws_secretsmanager_secret.near_mainnet_keypair.arn,
          data.aws_secretsmanager_secret.near_testnet_keypair.arn
        ]
      }
    ]
  })
}

# ECS Task Role (for application to access AWS services)
resource "aws_iam_role" "ecs_task" {
  name = "facilitator-${var.environment}-ecs-task"

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
    Name = "facilitator-${var.environment}-ecs-task"
  }
}

# ============================================================================
# ECS Cluster
# ============================================================================

resource "aws_ecs_cluster" "main" {
  name = "facilitator-${var.environment}"

  setting {
    name  = "containerInsights"
    value = var.enable_container_insights ? "enabled" : "disabled"
  }

  tags = {
    Name = "facilitator-${var.environment}"
  }
}

# ============================================================================
# ECS Task Definition
# ============================================================================

resource "aws_ecs_task_definition" "facilitator" {
  family                   = "facilitator-${var.environment}"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([
    {
      name      = "facilitator"
      image     = "${data.aws_caller_identity.current.account_id}.dkr.ecr.${data.aws_region.current.name}.amazonaws.com/${var.ecr_repository_name}:${var.image_tag}"
      essential = true

      portMappings = [
        {
          containerPort = 8080
          protocol      = "tcp"
        }
      ]

      environment = [
        {
          name  = "RUST_LOG"
          value = "info"
        },
        {
          name  = "SIGNER_TYPE"
          value = "private-key"
        },
        {
          name  = "PORT"
          value = "8080"
        },
        {
          name  = "HOST"
          value = "0.0.0.0"
        },
        {
          name  = "RPC_URL_CELO"
          value = "https://rpc.celocolombia.org"
        },
        {
          name  = "RPC_URL_BASE_SEPOLIA"
          value = "https://sepolia.base.org"
        },
        {
          name  = "RPC_URL_AVALANCHE_FUJI"
          value = "https://avalanche-fuji-c-chain-rpc.publicnode.com"
        },
        {
          name  = "RPC_URL_CELO_SEPOLIA"
          value = "https://rpc.ankr.com/celo_sepolia"
        },
        {
          name  = "RPC_URL_HYPEREVM_TESTNET"
          value = "https://rpc.hyperliquid-testnet.xyz/evm"
        },
        {
          name  = "RPC_URL_POLYGON_AMOY"
          value = "https://polygon-amoy.drpc.org"
        },
        {
          name  = "RPC_URL_OPTIMISM_SEPOLIA"
          value = "https://sepolia.optimism.io"
        },
        # ADDED: NEAR RPC endpoints
        {
          name  = "RPC_URL_NEAR_MAINNET"
          value = "https://rpc.mainnet.near.org"
        },
        {
          name  = "RPC_URL_NEAR_TESTNET"
          value = "https://rpc.testnet.near.org"
        }
      ]

      secrets = [
        {
          name      = "EVM_PRIVATE_KEY"
          valueFrom = "${data.aws_secretsmanager_secret.evm_private_key.arn}:private_key::"
        },
        {
          name      = "SOLANA_PRIVATE_KEY"
          valueFrom = "${data.aws_secretsmanager_secret.solana_keypair.arn}:private_key::"
        },
        {
          name      = "RPC_URL_BASE"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:base::"
        },
        {
          name      = "RPC_URL_AVALANCHE"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:avalanche::"
        },
        {
          name      = "RPC_URL_POLYGON"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:polygon::"
        },
        {
          name      = "RPC_URL_OPTIMISM"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:optimism::"
        },
        {
          name      = "RPC_URL_HYPEREVM"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:hyperevm::"
        },
        {
          name      = "RPC_URL_SOLANA"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:solana::"
        },
        {
          name      = "RPC_URL_SOLANA_DEVNET"
          valueFrom = "${data.aws_secretsmanager_secret.rpc_testnet.arn}:solana-devnet::"
        },
        # ADDED: NEAR private keys
        {
          name      = "NEAR_PRIVATE_KEY_MAINNET"
          valueFrom = "${data.aws_secretsmanager_secret.near_mainnet_keypair.arn}:private_key::"
        },
        {
          name      = "NEAR_PRIVATE_KEY_TESTNET"
          valueFrom = "${data.aws_secretsmanager_secret.near_testnet_keypair.arn}:private_key::"
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.facilitator.name
          "awslogs-region"        = data.aws_region.current.name
          "awslogs-stream-prefix" = "ecs"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "curl -f http://localhost:8080/health || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 60
      }
    }
  ])

  tags = {
    Name = "facilitator-${var.environment}"
  }
}

# ============================================================================
# ECS Service
# ============================================================================

resource "aws_ecs_service" "facilitator" {
  name            = "facilitator-${var.environment}"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.facilitator.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.ecs_tasks.id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.main.arn
    container_name   = "facilitator"
    container_port   = 8080
  }

  # Allow changes to task definition without destroying the service
  lifecycle {
    ignore_changes = [desired_count]
  }

  depends_on = [
    aws_lb_listener.https,
    aws_lb_listener.http
  ]

  tags = {
    Name = "facilitator-${var.environment}"
  }
}

# ============================================================================
# Auto Scaling
# ============================================================================

resource "aws_appautoscaling_target" "ecs_target" {
  max_capacity       = var.max_capacity
  min_capacity       = var.min_capacity
  resource_id        = "service/${aws_ecs_cluster.main.name}/${aws_ecs_service.facilitator.name}"
  scalable_dimension = "ecs:service:DesiredCount"
  service_namespace  = "ecs"
}

resource "aws_appautoscaling_policy" "ecs_cpu" {
  name               = "facilitator-${var.environment}-cpu"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.ecs_target.resource_id
  scalable_dimension = aws_appautoscaling_target.ecs_target.scalable_dimension
  service_namespace  = aws_appautoscaling_target.ecs_target.service_namespace

  target_tracking_scaling_policy_configuration {
    predefined_metric_specification {
      predefined_metric_type = "ECSServiceAverageCPUUtilization"
    }
    target_value = var.cpu_target_value
  }
}

resource "aws_appautoscaling_policy" "ecs_memory" {
  name               = "facilitator-${var.environment}-memory"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.ecs_target.resource_id
  scalable_dimension = aws_appautoscaling_target.ecs_target.scalable_dimension
  service_namespace  = aws_appautoscaling_target.ecs_target.service_namespace

  target_tracking_scaling_policy_configuration {
    predefined_metric_specification {
      predefined_metric_type = "ECSServiceAverageMemoryUtilization"
    }
    target_value = var.memory_target_value
  }
}
