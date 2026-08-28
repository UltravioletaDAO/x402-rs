# ============================================================================
# Facilitator Production Infrastructure - us-east-2
# ============================================================================
# Standalone deployment for facilitator.ultravioletadao.xyz
# Cost-optimized configuration: ~$43-48/month

# Data Sources
data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

# Route53 Hosted Zone (must already exist)
data "aws_route53_zone" "main" {
  name         = var.hosted_zone_name
  private_zone = false
}

# ============================================================================
# Secrets Manager References
# ============================================================================
# All secret data sources are now defined in secrets.tf
# This ensures consistency and makes it easy to add new networks

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

# NAT configuration:
#   single_nat_gateway = true   -> one NAT in AZ-0 (cheapest, AZ-0 outage drops egress for ALL private subnets)
#   single_nat_gateway = false  -> one NAT per AZ (multi-AZ resilience, ~$32/mo per extra NAT)
locals {
  nat_count = var.single_nat_gateway ? 1 : length(var.availability_zones)
}

# Elastic IPs for NAT (one per NAT gateway)
resource "aws_eip" "nat" {
  count  = local.nat_count
  domain = "vpc"

  tags = {
    Name = "facilitator-${var.environment}-nat-eip-${count.index}"
  }
}

# NAT Gateway(s) for private subnets to reach internet.
# Placed in the matching public subnet so traffic stays in-AZ when multi-AZ.
resource "aws_nat_gateway" "main" {
  count         = local.nat_count
  allocation_id = aws_eip.nat[count.index].id
  subnet_id     = aws_subnet.public[count.index].id

  tags = {
    Name = "facilitator-${var.environment}-nat-${count.index}"
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

# Route Table(s) for Private Subnets.
# When multi-AZ, each AZ gets its own RT pointing to its local NAT — so an AZ outage
# does not pull a healthy AZ's egress through a dead NAT.
resource "aws_route_table" "private" {
  count  = local.nat_count
  vpc_id = aws_vpc.main.id

  route {
    cidr_block     = "0.0.0.0/0"
    nat_gateway_id = aws_nat_gateway.main[count.index].id
  }

  tags = {
    Name = "facilitator-${var.environment}-private-rt-${count.index}"
  }
}

# Associate Public Subnets with Public Route Table
resource "aws_route_table_association" "public" {
  count          = length(aws_subnet.public)
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

# Associate Private Subnets with their AZ-local Private Route Table.
# Single-NAT mode: every subnet points at the only RT (index 0).
# Multi-AZ mode:   subnet N points at RT N (same AZ as its NAT).
resource "aws_route_table_association" "private" {
  count          = length(aws_subnet.private)
  subnet_id      = aws_subnet.private[count.index].id
  route_table_id = aws_route_table.private[var.single_nat_gateway ? 0 : count.index].id
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

  # B10: SG egress is no longer 0.0.0.0/0 0-65535/-1. RPC CIDRs are unenumerable
  # (chain providers run on diverse hosts and rotate IP ranges), so we cannot
  # literally allow-list specific destinations — but we CAN bound the protocol
  # surface so an RCE in the container cannot open arbitrary outbound sockets.
  # AWS-internal traffic (DynamoDB, Secrets Manager) goes through the VPC
  # endpoints declared below instead of the public internet.

  egress {
    description = "HTTPS to chain RPCs, AWS API endpoints, and the wider web"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "HTTP fallback for a few RPCs that still expose http://"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "DNS resolution (UDP)"
    from_port   = 53
    to_port     = 53
    protocol    = "udp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "DNS resolution (TCP fallback for large responses)"
    from_port   = 53
    to_port     = 53
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "NTP time sync"
    from_port   = 123
    to_port     = 123
    protocol    = "udp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name = "facilitator-${var.environment}-ecs-tasks-sg"
  }
}

# ============================================================================
# VPC Endpoints (B10 defense-in-depth)
# ============================================================================
# Route AWS-service traffic (DynamoDB, Secrets Manager) through private VPC
# endpoints instead of NAT gateway → public internet. This both reduces NAT
# costs/throughput and isolates the secrets/state plane from an
# arbitrary-internet-reachable code-execution scenario inside the container.

# DynamoDB Gateway endpoint — free, attached to the private route table(s).
# All traffic to DDB in this region from these subnets stays on the AWS
# backbone; no NAT hop, no internet edge.
resource "aws_vpc_endpoint" "dynamodb" {
  vpc_id            = aws_vpc.main.id
  service_name      = "com.amazonaws.${data.aws_region.current.name}.dynamodb"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = aws_route_table.private[*].id

  tags = {
    Name = "facilitator-${var.environment}-dynamodb-endpoint"
  }
}

# S3 Gateway endpoint — free, same deal as DynamoDB above.
# This one is not just defense-in-depth: the discovery health tracker rewrites
# the whole ~5.8 MB health.json overlay to S3 on every tick, and the resource
# catalog is a ~12.8 MB read-modify-write per registration. Without this
# endpoint every one of those bytes crossed the NAT gateway and got billed at
# $0.045/GB — ~277 GB/month, the single largest line item in NAT data
# processing. On the backbone it is free.
resource "aws_vpc_endpoint" "s3" {
  vpc_id            = aws_vpc.main.id
  service_name      = "com.amazonaws.${data.aws_region.current.name}.s3"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = aws_route_table.private[*].id

  tags = {
    Name = "facilitator-${var.environment}-s3-endpoint"
  }
}

# Security group dedicated to the Interface endpoints below. The endpoints
# expose ENIs inside our private subnets; this SG limits the source to the
# ECS task SG (port 443 only) so nothing else in the VPC can poke them.
resource "aws_security_group" "vpc_endpoints" {
  name        = "facilitator-${var.environment}-vpc-endpoints"
  description = "Security group for AWS service VPC interface endpoints"
  vpc_id      = aws_vpc.main.id

  ingress {
    description     = "HTTPS from facilitator ECS tasks"
    from_port       = 443
    to_port         = 443
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs_tasks.id]
  }

  egress {
    description = "Replies to ECS tasks (stateful SG; this rule is mostly for clarity)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = [var.vpc_cidr]
  }

  tags = {
    Name = "facilitator-${var.environment}-vpc-endpoints-sg"
  }
}

# Secrets Manager Interface endpoint. Costs ~$7/AZ/month + data charges.
# In single-NAT mode all egress is single-AZ anyway, so we pin the endpoint
# ENI to subnet[0] (the AZ that hosts the NAT) — one ENI, one charge.
# When the user switches `single_nat_gateway = false` we fan out the ENI
# across every private subnet for resilience.
resource "aws_vpc_endpoint" "secretsmanager" {
  vpc_id              = aws_vpc.main.id
  service_name        = "com.amazonaws.${data.aws_region.current.name}.secretsmanager"
  vpc_endpoint_type   = "Interface"
  subnet_ids          = var.single_nat_gateway ? [aws_subnet.private[0].id] : aws_subnet.private[*].id
  security_group_ids  = [aws_security_group.vpc_endpoints.id]
  private_dns_enabled = true

  tags = {
    Name = "facilitator-${var.environment}-secretsmanager-endpoint"
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
  enable_http2               = true
  idle_timeout               = var.alb_idle_timeout

  # Bucket referenced by NAME (local.alb_access_logs_bucket_name, a plain
  # string in alb-access-logs.tf), never by resource attribute. That is a
  # deliberate choice, not an oversight -- see var.alb_access_logs_enabled for
  # why a resource reference here would be dangerous. The dynamic block itself
  # (present/absent) is what CI's routine deploy diffs on, so it stays a no-op
  # until the flag flips.
  dynamic "access_logs" {
    for_each = var.alb_access_logs_enabled ? [1] : []
    content {
      bucket  = local.alb_access_logs_bucket_name
      prefix  = "alb"
      enabled = true
    }
  }

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

# Policy for accessing secrets
# Uses local.all_secret_arns from secrets.tf to ensure all secrets are accessible
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
        Resource = local.all_secret_arns
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

# DynamoDB table for nonce/replay protection (Stellar, Algorand)
resource "aws_dynamodb_table" "nonce_store" {
  name         = "facilitator-nonces"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"

  attribute {
    name = "pk"
    type = "S"
  }

  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  tags = {
    Name        = "facilitator-nonces"
    Environment = var.environment
  }
}

# Policy for DynamoDB nonce store access (task role)
resource "aws_iam_role_policy" "dynamodb_nonce_access" {
  name = "DynamoDBNonceStoreAccess"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:PutItem",
          "dynamodb:GetItem",
          "dynamodb:DescribeTable",
          # DeleteItem is what lets a claim be GIVEN BACK. The ERC-8004
          # proof gate claims the (payment, agent) pair before writing
          # on-chain -- claiming after would let two concurrent requests
          # both pass -- so when the write never lands the claim has to be
          # released or that payment could never buy its rating again.
          # Without this permission the release fails silently and the
          # caller is locked out by our own retry.
          "dynamodb:DeleteItem"
        ]
        Resource = aws_dynamodb_table.nonce_store.arn
      }
    ]
  })
}

# Historical index of every operation the facilitator handled.
#
# NOT a ledger: the write is fire-and-forget AFTER settlement resolves, so an
# outage here loses records and never blocks a payment. The chain stays the
# source of truth; this exists so "how much have we settled on Polygon" stops
# being a question you answer by grepping CloudWatch.
#
# Cost, measured 2026-07-30 rather than guessed: ~1,600 operations/day is about
# 48k writes/month = ~$0.06. The read side is where money is actually at risk,
# which is why aggregates live in their own partition and the stats page issues
# one bounded Query instead of scanning.
resource "aws_dynamodb_table" "transactions" {
  name         = "facilitator_transactions"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"
  range_key    = "sk"

  attribute {
    name = "pk"
    type = "S"
  }

  attribute {
    name = "sk"
    type = "S"
  }

  # Records carry expires_at; the aggregate items deliberately do NOT, so
  # lifetime totals survive the expiry of the rows that produced them.
  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  point_in_time_recovery {
    enabled = true
  }

  tags = {
    Name        = "facilitator-transactions"
    Environment = var.environment
  }
}

resource "aws_iam_role_policy" "dynamodb_transactions_access" {
  name = "DynamoDBTransactionStoreAccess"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:PutItem",
          "dynamodb:UpdateItem",
          "dynamodb:Query",
          "dynamodb:DescribeTable"
        ]
        # No Scan and no DeleteItem on purpose: a scan is the expensive mistake
        # this schema exists to avoid, and expiry is DynamoDB's job via TTL.
        Resource = aws_dynamodb_table.transactions.arn
      }
    ]
  })
}

# DynamoDB table for /settle idempotency-key cache.
# Holds the response_json for ~24h so a client retry with the same
# Idempotency-Key header returns the original response verbatim without
# re-running the on-chain settlement. TTL is enforced by DDB on the
# `expires_at` attribute (eventually consistent — handler also re-checks).
resource "aws_dynamodb_table" "idempotency_store" {
  name         = "idempotency_records"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "idempotency_key"

  attribute {
    name = "idempotency_key"
    type = "S"
  }

  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  tags = {
    Name        = "idempotency_records"
    Environment = var.environment
  }
}

# Policy for DynamoDB idempotency store access (task role)
resource "aws_iam_role_policy" "dynamodb_idempotency_access" {
  name = "DynamoDBIdempotencyStoreAccess"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:PutItem",
          "dynamodb:GetItem",
          "dynamodb:DescribeTable"
        ]
        Resource = aws_dynamodb_table.idempotency_store.arn
      }
    ]
  })
}

# Policy for S3 discovery store access (task role)
resource "aws_iam_role_policy" "s3_discovery_access" {
  name = "S3DiscoveryStoreAccess"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject"
        ]
        Resource = "arn:aws:s3:::facilitator-discovery-prod/*"
      },
      {
        Effect = "Allow"
        Action = [
          "s3:ListBucket",
          "s3:HeadBucket"
        ]
        Resource = "arn:aws:s3:::facilitator-discovery-prod"
      }
    ]
  })
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

# Capacity providers: FARGATE (default) + FARGATE_SPOT (for observability)
resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name       = aws_ecs_cluster.main.name
  capacity_providers = ["FARGATE", "FARGATE_SPOT"]

  default_capacity_provider_strategy {
    capacity_provider = "FARGATE"
    weight            = 1
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

  container_definitions = jsonencode(concat([
    {
      name = "facilitator"
      # The image comes from what is RUNNING, not from tfvars — see
      # local.facilitator_image. Applying the tfvars value blindly rolled
      # production back two months on 2026-08-03.
      image     = local.facilitator_image
      essential = true

      portMappings = [
        {
          containerPort = 8080
          protocol      = "tcp"
        }
      ]

      environment = concat([
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
        {
          name  = "NONCE_STORE_TABLE_NAME"
          value = aws_dynamodb_table.nonce_store.name
        },
        {
          name  = "IDEMPOTENCY_TABLE_NAME"
          value = aws_dynamodb_table.idempotency_store.name
        },
        # Additional network RPCs (public endpoints)
        {
          name  = "RPC_URL_MONAD"
          value = "https://rpc.monad.xyz"
        },
        {
          name  = "RPC_URL_FOGO"
          value = "https://rpc.fogo.nightly.app"
        },
        {
          name  = "RPC_URL_FOGO_TESTNET"
          value = "https://testnet.fogo.io"
        },
        {
          name = "RPC_URL_SUI"
          # NOT fullnode.mainnet.sui.io: that endpoint stopped serving JSON-RPC
          # entirely (-32601 "JSON-RPC on public fullnodes has been deprecated,
          # migrate to gRPC or GraphQL") on every method, so Sui mainnet was dead
          # in production until 2026-08-20 and nothing alarmed on it.
          value = "https://sui-rpc.publicnode.com"
        },
        {
          name = "RPC_URL_SUI_TESTNET"
          # Same deprecation applies to fullnode.testnet.sui.io.
          value = "https://sui-testnet-rpc.publicnode.com"
        },
        {
          name  = "RPC_URL_UNICHAIN_SEPOLIA"
          value = "https://unichain-sepolia.drpc.org"
        },
        {
          name  = "RPC_URL_BSC"
          value = "https://bsc-rpc.publicnode.com"
        },
        {
          name  = "RPC_URL_SKALE_BASE"
          value = "https://skale-base.skalenodes.com/v1/base"
        },
        {
          name  = "RPC_URL_SKALE_BASE_SEPOLIA"
          value = "https://base-sepolia-testnet.skalenodes.com/v1/jubilant-horrible-ancha"
        },
        {
          name  = "RPC_URL_SCROLL"
          value = "https://rpc.scroll.io"
        },
        {
          name  = "RPC_URL_ROBINHOOD"
          value = "https://rpc.mainnet.chain.robinhood.com"
        },
        {
          name  = "RPC_URL_ROBINHOOD_TESTNET"
          value = "https://rpc.testnet.chain.robinhood.com"
        },
        # Discovery API (Bazaar) configuration
        {
          name  = "DISCOVERY_S3_BUCKET"
          value = "facilitator-discovery-prod"
        },
        {
          name  = "DISCOVERY_S3_KEY"
          value = "bazaar/resources.json"
        },
        {
          name  = "FACILITATOR_URL"
          value = "https://facilitator.ultravioletadao.xyz"
        },
        # ============================================================
        # CRITICAL: Escrow Settlement - DO NOT DISABLE
        # ============================================================
        # This enables the escrow/settle endpoint for x402 payments.
        # Without this, ALL payment settlements will fail with error:
        # "Escrow settlement is disabled. Set ENABLE_ESCROW=true"
        #
        # WARNING: Never set this to "false" or remove this variable!
        # ============================================================
        {
          name  = "ENABLE_ESCROW"
          value = "true"
        },
        # ============================================================
        # PaymentOperator Escrow Scheme (x402r v2)
        # ============================================================
        # This enables the advanced escrow scheme using PaymentOperator
        # contracts (AuthCaptureEscrow + TokenCollector pattern).
        # Currently deployed only on Base Mainnet.
        # ============================================================
        {
          name  = "ENABLE_PAYMENT_OPERATOR"
          value = "true"
        },
        # ============================================================
        # Upto Scheme (Permit2-based variable amount settlement)
        # ============================================================
        # This enables the "upto" payment scheme where clients authorize
        # a maximum amount via Permit2 and the server settles for actual
        # usage (<= max). Uses x402UptoPermit2Proxy contract.
        # ============================================================
        {
          name  = "ENABLE_UPTO"
          value = "true"
        },
        # ============================================================
        # ERC-8004 reputation: authorship and the proof-of-payment gate
        # ============================================================
        # Written out explicitly even though they match the code defaults,
        # for the same reason as the events dial below: these decide who
        # can write reputation and under what evidence, and a decision
        # that lives only as a Rust default is one nobody can audit.
        #
        # Phase 1 of a two-phase rollout. With REQUIRE_PROOF=false the
        # facilitator runs every check on the ProofOfPayment attached to a
        # feedback (transaction exists and succeeded, right block, right
        # Transfer, payer == rater, payee tied to the agent, fresh,
        # paymentHash recomputes, not already spent) and REPORTS the
        # verdict without rejecting. That is how we measure how much real
        # traffic a hard gate would break BEFORE it breaks it. Flip to
        # "true" once the logs show the failures are gone.
        {
          name  = "ERC8004_REQUIRE_PROOF"
          value = "false"
        },
        # Seven days. Also the TTL of the anti-replay record, deliberately:
        # once a proof is too old to be accepted it no longer needs one.
        {
          name  = "ERC8004_PROOF_MAX_AGE_SECS"
          value = "604800"
        },
        # DEPRECATED authorship path, still open. Account 0 of the Solana
        # program's give_feedback instruction is the feedback AUTHOR, and
        # POST /feedback puts our keypair there - which is why 87,2% of the
        # feedback on Base is attributed to the facilitator's wallet. The
        # replacement is /feedback/solana/prepare + /feedback/solana/submit,
        # where the rater signs as `client` and we only pay the fee.
        # Set to "false" to close the old path once callers have migrated.
        {
          name  = "ERC8004_ALLOW_FACILITATOR_AUTHORSHIP"
          value = "true"
        },
        # How long a rater's EIP-7702 relay authorisation stays valid.
        # Short on purpose: `relayFeedback` is permissionless by design, so
        # a signed authorisation is live in the wild until its deadline.
        # Fifteen minutes is the mitigation agreed with Execution Market
        # for finding 4 of the delegate audit.
        {
          name  = "ERC8004_RELAY_DEADLINE_SECS"
          value = "900"
        },
        # ============================================================
        # Live traffic stream (GET /events, SSE) — EXPOSURE DIAL
        # ============================================================
        # These match the code defaults, and they are written out anyway
        # ON PURPOSE: this file is the canonical control surface for how
        # much the facilitator broadcasts, and an exposure decision that
        # lives only as a Rust default is a decision nobody can audit or
        # find.
        #
        # detail=full + scope=all means the stream carries the payer, tx
        # hash and amount of EVERY client of this facilitator, publicly
        # and without authentication — not just our own traffic. That is
        # a deliberate owner decision (Saul, 2026-07-28), taken with the
        # tradeoff on the table, against a recommendation to run minimal.
        # It is written here so that it stays a decision instead of
        # decaying into an accident.
        #
        # To narrow exposure WITHOUT a code change or a rebuild:
        #   X402_EVENTS_DETAIL = "minimal"  -> only {ts, kind, network, ok}
        #   X402_EVENTS_SCOPE  = "allowlist" + X402_EVENTS_ALLOWLIST
        #   X402_EVENTS_ENABLED = "false"   -> /events 404s entirely
        # ============================================================
        {
          name  = "X402_EVENTS_ENABLED"
          value = "true"
        },
        {
          name  = "X402_EVENTS_DETAIL"
          value = "full"
        },
        {
          name  = "X402_EVENTS_SCOPE"
          value = "all"
        },
        {
          # Publish operations that FAILED, not only the ones that resolved.
          #
          # While this was off, `/api/stats` reported settlesFailed=0 no matter
          # what happened — the store only ever saw successes. Measured worst
          # case on 2026-08-01: the panel showed "24 successes, 0 failures" for
          # an hour that was really 24 of 38, and a network that was failing
          # every single settle appeared nowhere at all, because a row is only
          # created on success. A dashboard that cannot express failure does not
          # read as incomplete, it reads as healthy.
          #
          # What goes out is a BOUNDED category (contract_revert,
          # invalid_signature, insufficient_funds, upstream_rpc_unavailable, …)
          # and never the raw error text: raw errors carry addresses and have
          # carried an RPC URL with its key inside it. See `failure_category`
          # in src/handlers.rs — anything unrecognised becomes "other" rather
          # than being echoed.
          #
          # Turning this off again restores the old silence; it does not stop
          # failures, only the reporting of them.
          name  = "X402_EVENTS_PUBLISH_FAILURES"
          value = "true"
        },
        {
          name  = "TRANSACTIONS_TABLE_NAME"
          value = aws_dynamodb_table.transactions.name
        },
        {
          # 0 keeps records forever. 90 days is a decision someone made rather
          # than an unbounded table nobody chose; aggregates never expire.
          name  = "TRANSACTIONS_TTL_DAYS"
          value = "90"
        }
        ], var.enable_dx402 ? [
        # ============================================================
        # DX402 -- durable-evidence extension
        # ============================================================
        # Off unless enable_dx402 is set. With these absent the /dx402/*
        # routes are never registered and /supported does not advertise
        # the extension, so the payment path is byte-for-byte unchanged.
        #
        # Missing or unusable config DISABLES the feature and logs why
        # (src/dx402/service.rs). It never falls back to an in-memory
        # store, which would report durable evidence for data that dies
        # with the process.
        # ============================================================
        {
          name  = "ENABLE_DX402"
          value = "true"
        },
        {
          name  = "DX402_STORE_BACKEND"
          value = var.dx402_storage_backend
        },
        {
          # The account's OWN gateway. Signing a private read URL against the
          # generic gateway.pinata.cloud answers 403, so this is not cosmetic.
          # Not a secret: it is a hostname, and reads still need a signed URL.
          name  = "DX402_PINATA_GATEWAY"
          value = var.dx402_pinata_gateway
        },
        {
          # Offering `ipfs-public` at all. See the variable: it is off because
          # the data made permanent belongs to the BUYER, not because a
          # credential is missing.
          name  = "DX402_ALLOW_PUBLIC_IPFS"
          value = tostring(var.dx402_allow_public_ipfs)
        },
        {
          name  = "DX402_STORE_BUCKET"
          value = local.dx402_bucket_name
        },
        {
          # Base for the pointers buyers receive. It points at the
          # facilitator's own blob route, NOT at S3: the bucket stays
          # private and this is the only way in. Safe to serve
          # unauthenticated because the bytes are sealed to the payer.
          #
          # Changing this breaks pointers already in the wild -- treat it
          # as permanent.
          name  = "DX402_STORE_PUBLIC_BASE"
          value = "https://facilitator.ultravioletadao.xyz/dx402/blob"
        },
        {
          name  = "DX402_REGISTRY_TABLE_NAME"
          value = local.dx402_table_name
        },
        {
          # Bounded on purpose. Anchoring is publishing, and `permanent`
          # is irrevocable -- a seller can still opt into it per route.
          name  = "DX402_RETENTION"
          value = "90d"
        },
        ] : [], var.enable_observability ? [
        # ============================================================
        # OpenTelemetry - Push to OTel Collector sidecar
        # (only when observability stack is enabled)
        # ============================================================
        {
          name  = "OTEL_EXPORTER_OTLP_ENDPOINT"
          value = "http://localhost:4318"
        },
        {
          name  = "OTEL_EXPORTER_OTLP_PROTOCOL"
          value = "http/protobuf"
        },
        {
          name  = "OTEL_SERVICE_NAME"
          value = "facilitator"
        }
      ] : [])

      # All secrets are defined in secrets.tf (local.all_task_secrets)
      # This ensures consistency and makes it impossible to forget a secret
      secrets = local.all_task_secrets

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
    ], var.enable_observability ? [
    # ============================================================
    # OpenTelemetry Collector Sidecar
    # (only deployed when observability stack is enabled)
    # ============================================================
    # Receives OTLP from facilitator on localhost:4318
    # Pushes metrics to Prometheus and traces to Tempo in the
    # observability task via Cloud Map service discovery.
    {
      name      = "otel-collector"
      image     = "${data.aws_caller_identity.current.account_id}.dkr.ecr.${data.aws_region.current.name}.amazonaws.com/facilitator-otel-collector:${var.otel_collector_image_tag}"
      essential = false

      # Hard memory limit: prevents OOM from killing the whole task.
      # The collector only forwards OTLP data, 256MB is generous.
      memory            = 256
      memoryReservation = 128

      portMappings = [
        {
          containerPort = 4317
          protocol      = "tcp"
        },
        {
          containerPort = 4318
          protocol      = "tcp"
        }
      ]

      environment = [
        {
          name  = "PROMETHEUS_REMOTE_WRITE_ENDPOINT"
          value = "http://observability.facilitator.local:9090/api/v1/write"
        },
        {
          name  = "TEMPO_OTLP_ENDPOINT"
          value = "observability.facilitator.local:4317"
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.facilitator.name
          "awslogs-region"        = data.aws_region.current.name
          "awslogs-stream-prefix" = "otel-collector"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "wget --spider -q http://localhost:13133 || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 15
      }
    }
  ] : []))

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

# Request-count-based scaling, not CPU.
#
# The 2026-08 performance diagnosis measured CPU never exceeding 25% across
# three separate degradation episodes -- the facilitator is I/O-bound (waiting
# on RPC calls), so a CPU target-tracking policy never fires for this
# workload. ALBRequestCountPerTarget actually tracks load: it is the average
# number of requests each running task received, over the ALB itself, and it
# is what caught the gap between "quiet" and "the incident is starting."
#
# See var.alb_request_count_target_value for how the target was calibrated.
resource "aws_appautoscaling_policy" "ecs_alb_request_count" {
  name               = "facilitator-${var.environment}-alb-request-count"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.ecs_target.resource_id
  scalable_dimension = aws_appautoscaling_target.ecs_target.scalable_dimension
  service_namespace  = aws_appautoscaling_target.ecs_target.service_namespace

  target_tracking_scaling_policy_configuration {
    predefined_metric_specification {
      predefined_metric_type = "ALBRequestCountPerTarget"
      resource_label         = "${aws_lb.main.arn_suffix}/${aws_lb_target_group.main.arn_suffix}"
    }
    target_value = var.alb_request_count_target_value
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
