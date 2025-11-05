# ============================================================================
# SECURITY GROUPS - Network Access Control
# ============================================================================

# ----------------------------------------------------------------------------
# ALB Security Group
# ----------------------------------------------------------------------------

resource "aws_security_group" "alb" {
  name_prefix = "${var.project_name}-${var.environment}-alb-"
  description = "Security group for Application Load Balancer"
  vpc_id      = aws_vpc.main.id

  # HTTP from anywhere (for agent API access)
  ingress {
    description = "HTTP from internet"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  # HTTPS from anywhere (if using SSL/TLS)
  ingress {
    description = "HTTPS from internet"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  # Allow all outbound traffic
  egress {
    description = "Allow all outbound"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-alb-sg"
  })

  lifecycle {
    create_before_destroy = true
  }
}

# ----------------------------------------------------------------------------
# ECS Tasks Security Group
# ----------------------------------------------------------------------------

resource "aws_security_group" "ecs_tasks" {
  name_prefix = "${var.project_name}-${var.environment}-ecs-tasks-"
  description = "Security group for ECS Fargate tasks"
  vpc_id      = aws_vpc.main.id

  # Allow traffic from ALB
  ingress {
    description     = "Traffic from ALB"
    from_port       = 0
    to_port         = 65535
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  # Allow inter-container communication (for Service Connect)
  # Agents need to call each other (e.g., skill-extractor -> karma-hello)
  ingress {
    description = "Inter-container communication"
    from_port   = 0
    to_port     = 65535
    protocol    = "tcp"
    self        = true
  }

  # Allow all outbound traffic (for blockchain RPC, OpenAI API, etc.)
  egress {
    description = "Allow all outbound"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-ecs-tasks-sg"
  })

  lifecycle {
    create_before_destroy = true
  }
}

# ----------------------------------------------------------------------------
# Security Group Rules for Specific Agent Ports
# ----------------------------------------------------------------------------
# These are additional rules to explicitly allow traffic to agent ports
# Useful for debugging and monitoring

resource "aws_security_group_rule" "validator_port" {
  description       = "Allow traffic to validator (9001)"
  type              = "ingress"
  from_port         = 9001
  to_port           = 9001
  protocol          = "tcp"
  security_group_id = aws_security_group.ecs_tasks.id
  source_security_group_id = aws_security_group.alb.id
}

resource "aws_security_group_rule" "karma_hello_port" {
  description       = "Allow traffic to karma-hello (9002)"
  type              = "ingress"
  from_port         = 9002
  to_port           = 9002
  protocol          = "tcp"
  security_group_id = aws_security_group.ecs_tasks.id
  source_security_group_id = aws_security_group.alb.id
}

resource "aws_security_group_rule" "abracadabra_port" {
  description       = "Allow traffic to abracadabra (9003)"
  type              = "ingress"
  from_port         = 9003
  to_port           = 9003
  protocol          = "tcp"
  security_group_id = aws_security_group.ecs_tasks.id
  source_security_group_id = aws_security_group.alb.id
}

resource "aws_security_group_rule" "skill_extractor_port" {
  description       = "Allow traffic to skill-extractor (9004)"
  type              = "ingress"
  from_port         = 9004
  to_port           = 9004
  protocol          = "tcp"
  security_group_id = aws_security_group.ecs_tasks.id
  source_security_group_id = aws_security_group.alb.id
}

resource "aws_security_group_rule" "voice_extractor_port" {
  description       = "Allow traffic to voice-extractor (9005)"
  type              = "ingress"
  from_port         = 9005
  to_port           = 9005
  protocol          = "tcp"
  security_group_id = aws_security_group.ecs_tasks.id
  source_security_group_id = aws_security_group.alb.id
}

# ----------------------------------------------------------------------------
# Optional: Security Group for Debugging (SSH/Session Manager)
# ----------------------------------------------------------------------------
# Uncomment to enable SSH access for debugging (not recommended for production)

# resource "aws_security_group_rule" "ssh_access" {
#   description       = "SSH access for debugging"
#   type              = "ingress"
#   from_port         = 22
#   to_port           = 22
#   protocol          = "tcp"
#   security_group_id = aws_security_group.ecs_tasks.id
#   cidr_blocks       = ["YOUR_IP/32"] # Replace with your IP
# }
