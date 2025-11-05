# ============================================================================
# ROUTE53 DNS RECORDS - Karmacadabra Agent Domains
# ============================================================================
# Creates DNS records for each agent at karmacadabra.ultravioletadao.xyz
#
# Domain structure:
# - Base: karmacadabra.ultravioletadao.xyz → ALB
# - Agents: <agent>.karmacadabra.ultravioletadao.xyz → ALB
#
# Examples:
# - validator.karmacadabra.ultravioletadao.xyz
# - karma-hello.karmacadabra.ultravioletadao.xyz
# - abracadabra.karmacadabra.ultravioletadao.xyz

# ----------------------------------------------------------------------------
# Data Source: Existing Hosted Zone
# ----------------------------------------------------------------------------

data "aws_route53_zone" "main" {
  count = var.enable_route53 ? 1 : 0

  name         = var.hosted_zone_name
  private_zone = false
}

# ----------------------------------------------------------------------------
# Base Domain Record (karmacadabra.ultravioletadao.xyz)
# ----------------------------------------------------------------------------

resource "aws_route53_record" "base" {
  count = var.enable_route53 ? 1 : 0

  zone_id = data.aws_route53_zone.main[0].zone_id
  name    = var.base_domain
  type    = "A"

  alias {
    name                   = aws_lb.main.dns_name
    zone_id                = aws_lb.main.zone_id
    evaluate_target_health = true
  }
}

# ----------------------------------------------------------------------------
# Agent Subdomain Records
# ----------------------------------------------------------------------------
# Creates DNS records for each agent:
# - validator.karmacadabra.ultravioletadao.xyz
# - karma-hello.karmacadabra.ultravioletadao.xyz
# - abracadabra.karmacadabra.ultravioletadao.xyz
# - skill-extractor.karmacadabra.ultravioletadao.xyz
# - voice-extractor.karmacadabra.ultravioletadao.xyz

resource "aws_route53_record" "agents" {
  for_each = var.enable_route53 ? var.agents : {}

  zone_id = data.aws_route53_zone.main[0].zone_id
  name    = "${each.key}.${var.base_domain}"
  type    = "A"

  alias {
    name                   = aws_lb.main.dns_name
    zone_id                = aws_lb.main.zone_id
    evaluate_target_health = true
  }
}

# ----------------------------------------------------------------------------
# Facilitator Record (facilitator.ultravioletadao.xyz)
# ----------------------------------------------------------------------------
# Special case: facilitator sits at root domain, not under karmacadabra

resource "aws_route53_record" "facilitator" {
  count = var.enable_route53 ? 1 : 0

  zone_id = data.aws_route53_zone.main[0].zone_id
  name    = "facilitator.${var.hosted_zone_name}"
  type    = "A"

  alias {
    name                   = aws_lb.main.dns_name
    zone_id                = aws_lb.main.zone_id
    evaluate_target_health = true
  }
}

# ----------------------------------------------------------------------------
# Test Seller Record (test-seller.karmacadabra.ultravioletadao.xyz)
# ----------------------------------------------------------------------------
# Note: This is already covered by the wildcard *.karmacadabra.ultravioletadao.xyz
# and also by the agents loop in route53.tf, but we keep it explicit for clarity

# Commented out - using the automatic agent subdomain from agents map instead
# resource "aws_route53_record" "test_seller" {
#   count = var.enable_route53 ? 1 : 0
#
#   zone_id = data.aws_route53_zone.main[0].zone_id
#   name    = "test-seller.${var.base_domain}"
#   type    = "A"
#
#   alias {
#     name                   = aws_lb.main.dns_name
#     zone_id                = aws_lb.main.zone_id
#     evaluate_target_health = true
#   }
# }

# ----------------------------------------------------------------------------
# Wildcard Record (Optional)
# ----------------------------------------------------------------------------
# Uncomment to create *.karmacadabra.ultravioletadao.xyz → ALB
# Useful for adding new agents without updating DNS

# resource "aws_route53_record" "wildcard" {
#   count = var.enable_route53 && var.enable_wildcard_domain ? 1 : 0
#
#   zone_id = data.aws_route53_zone.main[0].zone_id
#   name    = "*.${var.base_domain}"
#   type    = "A"
#
#   alias {
#     name                   = aws_lb.main.dns_name
#     zone_id                = aws_lb.main.zone_id
#     evaluate_target_health = true
#   }
# }
