# ============================================================================
# APPLICATION LOAD BALANCER - Cost-Optimized Configuration
# ============================================================================
# COST: ~$16-18/month for ALB + data transfer
# OPTIMIZATION: Path-based routing to single ALB (vs 5 separate ALBs)

# ----------------------------------------------------------------------------
# Application Load Balancer
# ----------------------------------------------------------------------------

resource "aws_lb" "main" {
  name               = "${var.project_name}-${var.environment}-alb"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb.id]
  subnets            = aws_subnet.public[*].id

  enable_deletion_protection = var.alb_deletion_protection
  enable_http2               = true
  enable_cross_zone_load_balancing = true
  idle_timeout               = var.alb_idle_timeout

  # Access logs (disabled by default to save S3 costs)
  dynamic "access_logs" {
    for_each = var.enable_alb_access_logs ? [1] : []
    content {
      bucket  = aws_s3_bucket.alb_logs[0].id
      enabled = true
    }
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-alb"
  })
}

# ----------------------------------------------------------------------------
# Target Groups (one per agent)
# ----------------------------------------------------------------------------

resource "aws_lb_target_group" "agents" {
  for_each = var.agents

  name_prefix = substr("${each.key}-", 0, 6)
  port        = each.value.port
  protocol    = "HTTP"
  vpc_id      = aws_vpc.main.id
  target_type = "ip" # Required for Fargate

  # Health check configuration
  health_check {
    enabled             = true
    healthy_threshold   = var.health_check_healthy_threshold
    unhealthy_threshold = var.health_check_unhealthy_threshold
    timeout             = var.health_check_timeout
    interval            = var.health_check_interval
    path                = each.value.health_check_path
    protocol            = "HTTP"
    matcher             = "200"
  }

  # Deregistration delay (how long to wait before removing targets)
  deregistration_delay = 30

  # Stickiness (optional - disabled to save processing costs)
  # stickiness {
  #   type            = "lb_cookie"
  #   cookie_duration = 86400
  #   enabled         = false
  # }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-tg"
    Agent = each.key
  })

  lifecycle {
    create_before_destroy = true
  }
}

# ----------------------------------------------------------------------------
# HTTP Listener (Port 80)
# ----------------------------------------------------------------------------

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.main.arn
  port              = 80
  protocol          = "HTTP"

  # Default action: redirect to HTTPS or return 404
  default_action {
    type = var.enable_https && var.redirect_http_to_https ? "redirect" : "fixed-response"

    dynamic "redirect" {
      for_each = var.enable_https && var.redirect_http_to_https ? [1] : []
      content {
        port        = "443"
        protocol    = "HTTPS"
        status_code = "HTTP_301"
      }
    }

    dynamic "fixed_response" {
      for_each = var.enable_https && var.redirect_http_to_https ? [] : [1]
      content {
        content_type = "application/json"
        message_body = jsonencode({
          error   = "Not Found"
          message = "No agent matched the request path"
          agents = {
            validator       = "/validator/*"
            karma-hello     = "/karma-hello/*"
            abracadabra     = "/abracadabra/*"
            skill-extractor = "/skill-extractor/*"
            voice-extractor = "/voice-extractor/*"
          }
        })
        status_code = "404"
      }
    }
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-http-listener"
  })
}

# ----------------------------------------------------------------------------
# Listener Rules (Path-based routing to agents)
# ----------------------------------------------------------------------------

resource "aws_lb_listener_rule" "agents_path" {
  for_each = var.agents

  listener_arn = aws_lb_listener.http.arn
  priority     = each.value.priority

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.agents[each.key].arn
  }

  condition {
    path_pattern {
      values = ["/${each.key}/*"]
    }
  }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-path-rule"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# Listener Rules (Hostname-based routing to agents)
# ----------------------------------------------------------------------------
# Routes requests based on Host header:
# - validator.karmacadabra.ultravioletadao.xyz → validator
# - karma-hello.karmacadabra.ultravioletadao.xyz → karma-hello
# etc.

resource "aws_lb_listener_rule" "agents_hostname" {
  for_each = var.enable_hostname_routing ? var.agents : {}

  listener_arn = aws_lb_listener.http.arn
  priority     = each.value.priority + 1000 # Higher priority than path-based

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.agents[each.key].arn
  }

  condition {
    host_header {
      values = ["${each.key}.${var.base_domain}"]
    }
  }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-hostname-rule"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# HTTPS Listener (Port 443)
# ----------------------------------------------------------------------------

resource "aws_lb_listener" "https" {
  count = var.enable_https ? 1 : 0

  load_balancer_arn = aws_lb.main.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = var.ssl_policy
  certificate_arn   = aws_acm_certificate_validation.main[0].certificate_arn

  default_action {
    type = "fixed-response"

    fixed_response {
      content_type = "application/json"
      message_body = jsonencode({
        error   = "Not Found"
        message = "No agent matched the request path"
        agents = {
          validator       = "/validator/*"
          karma-hello     = "/karma-hello/*"
          abracadabra     = "/abracadabra/*"
          skill-extractor = "/skill-extractor/*"
          voice-extractor = "/voice-extractor/*"
        }
      })
      status_code = "404"
    }
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-https-listener"
  })
}

# ----------------------------------------------------------------------------
# HTTPS Listener Rules (Path-based routing)
# ----------------------------------------------------------------------------

resource "aws_lb_listener_rule" "agents_path_https" {
  for_each = var.enable_https ? var.agents : {}

  listener_arn = aws_lb_listener.https[0].arn
  priority     = each.value.priority

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.agents[each.key].arn
  }

  condition {
    path_pattern {
      values = ["/${each.key}/*"]
    }
  }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-path-https-rule"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# HTTPS Listener Rules (Hostname-based routing)
# ----------------------------------------------------------------------------

resource "aws_lb_listener_rule" "agents_hostname_https" {
  for_each = var.enable_https && var.enable_hostname_routing ? var.agents : {}

  listener_arn = aws_lb_listener.https[0].arn
  priority     = each.value.priority + 1000

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.agents[each.key].arn
  }

  condition {
    host_header {
      values = ["${each.key}.${var.base_domain}"]
    }
  }

  tags = merge(var.tags, {
    Name  = "${var.project_name}-${var.environment}-${each.key}-hostname-https-rule"
    Agent = each.key
  })
}

# ----------------------------------------------------------------------------
# Facilitator Root Domain Routing (facilitator.ultravioletadao.xyz)
# ----------------------------------------------------------------------------

resource "aws_lb_listener_rule" "facilitator_root_http" {
  listener_arn = aws_lb_listener.http.arn
  priority     = 10 # High priority

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.agents["facilitator"].arn
  }

  condition {
    host_header {
      values = ["facilitator.${var.hosted_zone_name}"]
    }
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-facilitator-root-http"
  })
}

resource "aws_lb_listener_rule" "facilitator_root_https" {
  count = var.enable_https ? 1 : 0

  listener_arn = aws_lb_listener.https[0].arn
  priority     = 10 # High priority

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.agents["facilitator"].arn
  }

  condition {
    host_header {
      values = ["facilitator.${var.hosted_zone_name}"]
    }
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-facilitator-root-https"
  })
}

# ----------------------------------------------------------------------------
# Test Seller Domain Routing (test-seller.karmacadabra.ultravioletadao.xyz)
# ----------------------------------------------------------------------------
# Note: This is already covered by the agents_hostname rules above,
# but we keep it explicit for clarity and to ensure priority

# Commented out - using the automatic agent hostname routing instead
# The agents_hostname resource already creates rules for test-seller.karmacadabra.ultravioletadao.xyz

# resource "aws_lb_listener_rule" "test_seller_http" {
#   listener_arn = aws_lb_listener.http.arn
#   priority     = 15 # High priority (after facilitator)
#
#   action {
#     type             = "forward"
#     target_group_arn = aws_lb_target_group.agents["test-seller"].arn
#   }
#
#   condition {
#     host_header {
#       values = ["test-seller.${var.base_domain}"]
#     }
#   }
#
#   tags = merge(var.tags, {
#     Name = "${var.project_name}-${var.environment}-test-seller-http"
#   })
# }
#
# resource "aws_lb_listener_rule" "test_seller_https" {
#   count = var.enable_https ? 1 : 0
#
#   listener_arn = aws_lb_listener.https[0].arn
#   priority     = 15 # High priority (after facilitator)
#
#   action {
#     type             = "forward"
#     target_group_arn = aws_lb_target_group.agents["test-seller"].arn
#   }
#
#   condition {
#     host_header {
#       values = ["test-seller.${var.base_domain}"]
#     }
#   }
#
#   tags = merge(var.tags, {
#     Name = "${var.project_name}-${var.environment}-test-seller-https"
#   })
# }

# ----------------------------------------------------------------------------
# S3 Bucket for ALB Access Logs (Optional)
# ----------------------------------------------------------------------------

resource "aws_s3_bucket" "alb_logs" {
  count = var.enable_alb_access_logs ? 1 : 0

  bucket_prefix = "${var.project_name}-${var.environment}-alb-logs-"

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-alb-logs"
  })
}

resource "aws_s3_bucket_lifecycle_configuration" "alb_logs" {
  count = var.enable_alb_access_logs ? 1 : 0

  bucket = aws_s3_bucket.alb_logs[0].id

  rule {
    id     = "delete-old-logs"
    status = "Enabled"

    filter {} # Apply to all objects

    expiration {
      days = 7 # Keep logs for 7 days only
    }
  }
}

resource "aws_s3_bucket_public_access_block" "alb_logs" {
  count = var.enable_alb_access_logs ? 1 : 0

  bucket = aws_s3_bucket.alb_logs[0].id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_policy" "alb_logs" {
  count = var.enable_alb_access_logs ? 1 : 0

  bucket = aws_s3_bucket.alb_logs[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Principal = {
          AWS = "arn:aws:iam::127311923021:root" # ELB service account for us-east-1
        }
        Action   = "s3:PutObject"
        Resource = "${aws_s3_bucket.alb_logs[0].arn}/*"
      }
    ]
  })
}
