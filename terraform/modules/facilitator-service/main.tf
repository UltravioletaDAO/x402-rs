# ============================================================================
# ECS FARGATE CLUSTER - Main Infrastructure
# ============================================================================
# COST-OPTIMIZED CONFIGURATION:
# - Fargate Spot (70% cheaper than on-demand)
# - Mixed task sizes:
#   * Facilitator: 1 vCPU / 2GB RAM (handles blockchain transactions)
#   * Other agents: 0.25 vCPU / 0.5GB RAM (lightweight services)
# - Conservative auto-scaling (max 3 tasks per service)
# - Container Insights enabled (essential observability)
# - Service Connect for inter-agent communication (no ALB needed)
#
# MONTHLY COST ESTIMATE:
# - Facilitator (1 vCPU / 2GB): ~$12-15/month (Spot)
# - 5 agents x $1.50/month (Spot) = ~$7.50/month
# - ALB: ~$16/month
# - NAT Gateway: ~$32/month
# - CloudWatch Logs (7 days): ~$5/month
# - Container Insights: ~$3/month
# TOTAL: ~$75-92/month (can be reduced further by scaling down)

terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  # Backend configuration for remote state in S3
  backend "s3" {
    bucket         = "karmacadabra-terraform-state"
    key            = "ecs-fargate/terraform.tfstate"
    region         = "us-east-1"
    encrypt        = true
    dynamodb_table = "karmacadabra-terraform-locks"
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = var.tags
  }
}

# ----------------------------------------------------------------------------
# Data Sources
# ----------------------------------------------------------------------------

data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

# Data sources for individual agent secrets (flat JSON structure)
# Facilitator uses network-specific secret naming: karmacadabra-facilitator-mainnet
# All other agents use: karmacadabra-{agent-name}
data "aws_secretsmanager_secret" "agent_secrets" {
  for_each = var.agents
  name     = each.key == "facilitator" ? "karmacadabra-facilitator-mainnet" : "karmacadabra-${each.key}"
}

# Solana keypair secret for facilitator
data "aws_secretsmanager_secret" "solana_keypair" {
  name = "karmacadabra-solana-keypair"
}

# ----------------------------------------------------------------------------
# ECS Cluster
# ----------------------------------------------------------------------------

resource "aws_ecs_cluster" "main" {
  name = "${var.project_name}-${var.environment}"

  setting {
    name  = "containerInsights"
    value = var.enable_container_insights ? "enabled" : "disabled"
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-cluster"
  })
}

# ----------------------------------------------------------------------------
# ECS Cluster Capacity Providers (CRITICAL FOR COST)
# ----------------------------------------------------------------------------
# Fargate Spot provides 70% cost savings vs on-demand

resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name = aws_ecs_cluster.main.name

  capacity_providers = var.use_fargate_spot ? ["FARGATE_SPOT", "FARGATE"] : ["FARGATE"]

  default_capacity_provider_strategy {
    capacity_provider = var.use_fargate_spot ? "FARGATE_SPOT" : "FARGATE"
    weight            = var.use_fargate_spot ? var.fargate_spot_weight : 100
    base              = var.fargate_spot_base_capacity
  }

  dynamic "default_capacity_provider_strategy" {
    for_each = var.use_fargate_spot ? [1] : []
    content {
      capacity_provider = "FARGATE"
      weight            = var.fargate_ondemand_weight
    }
  }
}

# ----------------------------------------------------------------------------
# Service Discovery Namespace (for Service Connect)
# ----------------------------------------------------------------------------

resource "aws_service_discovery_private_dns_namespace" "main" {
  count = var.enable_service_connect ? 1 : 0

  name = var.service_connect_namespace
  vpc  = aws_vpc.main.id

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-service-discovery"
  })
}

# ----------------------------------------------------------------------------
# ECS Task Definitions (one per agent)
# ----------------------------------------------------------------------------

resource "aws_ecs_task_definition" "agents" {
  for_each = var.agents

  family                   = "${var.project_name}-${var.environment}-${each.key}"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = each.key == "facilitator" ? var.facilitator_task_cpu : var.task_cpu
  memory                   = each.key == "facilitator" ? var.facilitator_task_memory : var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  # Container definition
  container_definitions = jsonencode([
    {
      name  = each.key
      image = "${aws_ecr_repository.agents[each.key].repository_url}:latest"

      essential = true

      portMappings = [
        {
          containerPort = each.value.port
          hostPort      = each.value.port
          protocol      = "tcp"
          name          = each.key # For Service Connect
        }
      ]

      # Environment variables (non-secret)
      # Facilitator (Rust) vs Python agents have different requirements
      environment = each.key == "facilitator" ? [
        {
          name  = "PORT"
          value = tostring(each.value.port)
        },
        {
          name  = "HOST"
          value = "0.0.0.0"
        },
        {
          name  = "RUST_LOG"
          value = "info"
        },
        {
          name  = "SIGNER_TYPE"
          value = "private-key"
        },
        {
          name  = "RPC_URL_AVALANCHE_FUJI"
          value = "https://avalanche-fuji-c-chain-rpc.publicnode.com"
        },
        {
          name  = "RPC_URL_BASE_SEPOLIA"
          value = "https://sepolia.base.org"
        },
        {
          name  = "RPC_URL_BASE"
          value = "https://mainnet.base.org"
        },
        {
          name  = "RPC_URL_AVALANCHE"
          value = "https://avalanche-c-chain-rpc.publicnode.com"
        },
        {
          name  = "RPC_URL_CELO"
          value = "https://rpc.celocolombia.org"
        },
        {
          name  = "RPC_URL_CELO_SEPOLIA"
          value = "https://rpc.ankr.com/celo_sepolia"
        },
        {
          name  = "RPC_URL_HYPEREVM"
          value = "https://rpc.hyperliquid.xyz/evm"
        },
        {
          name  = "RPC_URL_HYPEREVM_TESTNET"
          value = "https://rpc.hyperliquid-testnet.xyz/evm"
        },
        {
          name  = "RPC_URL_SOLANA"
          value = "https://api.mainnet-beta.solana.com"
        },
        {
          name  = "RPC_URL_POLYGON"
          value = "https://polygon.drpc.org"
        },
        {
          name  = "RPC_URL_POLYGON_AMOY"
          value = "https://polygon-amoy.drpc.org"
        },
        {
          name  = "RPC_URL_OPTIMISM"
          value = "https://public-op-mainnet.fastnode.io"
        },
        {
          name  = "RPC_URL_OPTIMISM_SEPOLIA"
          value = "https://sepolia.optimism.io"
        },
        {
          name  = "GLUE_TOKEN_ADDRESS"
          value = "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743"
        },
        {
          name  = "GLUE_TOKEN_ADDRESS_AVALANCHE_FUJI"
          value = "0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743"
        },
        {
          name  = "USDC_FUJI_ADDRESS"
          value = "0x5425890298aed601595a70AB815c96711a31Bc65"
        },
        {
          name  = "WAVAX_FUJI_ADDRESS"
          value = "0xd00ae08403B9bbb9124bB305C09058E32C39A48c"
        }
      ] : [
        {
          name  = "PORT"
          value = tostring(each.value.port)
        },
        {
          name  = "PYTHONPATH"
          value = "/app"
        },
        {
          name  = "USE_LOCAL_FILES"
          value = "true"
        },
        {
          name  = "AGENT_NAME"
          value = each.key
        },
        {
          name  = "ENVIRONMENT"
          value = var.environment
        },
        {
          name  = "AWS_REGION"
          value = var.aws_region
        }
      ]

      # Secrets from AWS Secrets Manager
      # Format: arn:...:secret-name:json-key::
      # Facilitator uses EVM_PRIVATE_KEY + SOLANA_PRIVATE_KEY (no OpenAI), agents use PRIVATE_KEY + OPENAI_API_KEY
      secrets = each.key == "facilitator" ? [
        {
          name      = "EVM_PRIVATE_KEY"
          valueFrom = "${data.aws_secretsmanager_secret.agent_secrets[each.key].arn}:private_key::"
        },
        {
          name      = "SOLANA_PRIVATE_KEY"
          valueFrom = "${data.aws_secretsmanager_secret.solana_keypair.arn}:private_key::"
        }
      ] : [
        {
          name      = "PRIVATE_KEY"
          valueFrom = "${data.aws_secretsmanager_secret.agent_secrets[each.key].arn}:private_key::"
        },
        {
          name      = "OPENAI_API_KEY"
          valueFrom = "${data.aws_secretsmanager_secret.agent_secrets[each.key].arn}:openai_api_key::"
        }
      ]

      # Health check
      healthCheck = {
        command = [
          "CMD-SHELL",
          "curl -f http://localhost:${each.value.port}${each.value.health_check_path} || exit 1"
        ]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 60
      }

      # Logging configuration
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.agents[each.key].name
          "awslogs-region"        = var.aws_region
          "awslogs-stream-prefix" = "ecs"
        }
      }

      # X-Ray sidecar (optional)
      # Uncomment to enable X-Ray tracing
      # {
      #   name      = "xray-daemon"
      #   image     = "amazon/aws-xray-daemon"
      #   cpu       = 32
      #   memory    = 256
      #   essential = false
      #   portMappings = [{
      #     containerPort = 2000
      #     protocol      = "udp"
      #   }]
      # }
    }
  ])

  # Runtime platform
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-task"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# ECS Services (one per agent)
# ----------------------------------------------------------------------------

resource "aws_ecs_service" "agents" {
  for_each = var.agents

  name            = "${var.project_name}-${var.environment}-${each.key}"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.agents[each.key].arn
  desired_count   = var.desired_count_per_service
  launch_type     = null # Use capacity provider strategy instead

  # Capacity provider strategy
  # Facilitator uses on-demand (more stable), other agents use Spot (cheaper)
  dynamic "capacity_provider_strategy" {
    for_each = each.key == "facilitator" ? [] : (var.use_fargate_spot ? [1] : [])
    content {
      capacity_provider = "FARGATE_SPOT"
      weight            = var.fargate_spot_weight
      base              = var.fargate_spot_base_capacity
    }
  }

  dynamic "capacity_provider_strategy" {
    for_each = each.key == "facilitator" ? [1] : (var.use_fargate_spot ? [1] : [])
    content {
      capacity_provider = "FARGATE"
      weight            = each.key == "facilitator" ? 100 : var.fargate_ondemand_weight
      base              = each.key == "facilitator" ? 1 : 0
    }
  }

  # Deployment configuration
  # Note: deployment_configuration removed temporarily due to provider compatibility
  # Will be added back once syntax is confirmed for AWS provider 5.x
  # deployment_configuration {
  #   maximum_percent         = 200
  #   minimum_healthy_percent = 100
  # }

  # Network configuration
  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.ecs_tasks.id]
    assign_public_ip = false # Private subnet, use NAT for outbound
  }

  # Load balancer configuration
  load_balancer {
    target_group_arn = aws_lb_target_group.agents[each.key].arn
    container_name   = each.key
    container_port   = each.value.port
  }

  # Service Connect (for inter-agent communication)
  dynamic "service_connect_configuration" {
    for_each = var.enable_service_connect ? [1] : []
    content {
      enabled   = true
      namespace = aws_service_discovery_private_dns_namespace.main[0].arn

      service {
        port_name      = each.key
        discovery_name = each.key
        client_alias {
          port     = each.value.port
          dns_name = each.key
        }
      }
    }
  }

  # Enable ECS Exec for debugging
  enable_execute_command = var.enable_execute_command

  # Health check grace period
  health_check_grace_period_seconds = 60

  # Force new deployment on task definition change
  force_new_deployment = true

  # Propagate tags from task definition
  propagate_tags = "TASK_DEFINITION"

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-service"
    Agent = each.key
  })

  depends_on = [
    aws_lb_listener.http,
    aws_iam_role_policy.ecs_secrets_access,
    aws_iam_role_policy.task_secrets_access
  ]
}

# ----------------------------------------------------------------------------
# Auto-Scaling Targets
# ----------------------------------------------------------------------------

resource "aws_appautoscaling_target" "ecs_service" {
  for_each = var.enable_autoscaling ? var.agents : {}

  max_capacity       = var.autoscaling_max_capacity
  min_capacity       = var.autoscaling_min_capacity
  resource_id        = "service/${aws_ecs_cluster.main.name}/${aws_ecs_service.agents[each.key].name}"
  scalable_dimension = "ecs:service:DesiredCount"
  service_namespace  = "ecs"
}

# ----------------------------------------------------------------------------
# Auto-Scaling Policies - CPU-based
# ----------------------------------------------------------------------------

resource "aws_appautoscaling_policy" "cpu" {
  for_each = var.enable_autoscaling ? var.agents : {}

  name               = "${var.project_name}-${var.environment}-${each.key}-cpu-scaling"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.ecs_service[each.key].resource_id
  scalable_dimension = aws_appautoscaling_target.ecs_service[each.key].scalable_dimension
  service_namespace  = aws_appautoscaling_target.ecs_service[each.key].service_namespace

  target_tracking_scaling_policy_configuration {
    target_value       = var.autoscaling_cpu_target
    scale_in_cooldown  = 300 # 5 minutes
    scale_out_cooldown = 60  # 1 minute

    predefined_metric_specification {
      predefined_metric_type = "ECSServiceAverageCPUUtilization"
    }
  }
}

# ----------------------------------------------------------------------------
# Auto-Scaling Policies - Memory-based
# ----------------------------------------------------------------------------

resource "aws_appautoscaling_policy" "memory" {
  for_each = var.enable_autoscaling ? var.agents : {}

  name               = "${var.project_name}-${var.environment}-${each.key}-memory-scaling"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.ecs_service[each.key].resource_id
  scalable_dimension = aws_appautoscaling_target.ecs_service[each.key].scalable_dimension
  service_namespace  = aws_appautoscaling_target.ecs_service[each.key].service_namespace

  target_tracking_scaling_policy_configuration {
    target_value       = var.autoscaling_memory_target
    scale_in_cooldown  = 300 # 5 minutes
    scale_out_cooldown = 60  # 1 minute

    predefined_metric_specification {
      predefined_metric_type = "ECSServiceAverageMemoryUtilization"
    }
  }
}
