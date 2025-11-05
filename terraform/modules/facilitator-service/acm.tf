# ============================================================================
# ACM CERTIFICATE - SSL/TLS for HTTPS
# ============================================================================
# Creates and validates SSL certificate for karmacadabra domains

# ----------------------------------------------------------------------------
# ACM Certificate
# ----------------------------------------------------------------------------

resource "aws_acm_certificate" "main" {
  count = var.enable_https ? 1 : 0

  domain_name               = var.base_domain
  subject_alternative_names = [
    "*.${var.base_domain}",                # Wildcard for all agent subdomains
    "facilitator.${var.hosted_zone_name}"  # Facilitator at root domain
  ]
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }

  tags = merge(var.tags, {
    Name = "${var.project_name}-${var.environment}-cert"
  })
}

# ----------------------------------------------------------------------------
# Route53 DNS Validation Records
# ----------------------------------------------------------------------------

resource "aws_route53_record" "cert_validation" {
  for_each = var.enable_https && var.enable_route53 ? {
    for dvo in aws_acm_certificate.main[0].domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  } : {}

  allow_overwrite = true
  name            = each.value.name
  records         = [each.value.record]
  ttl             = 60
  type            = each.value.type
  zone_id         = data.aws_route53_zone.main[0].zone_id
}

# ----------------------------------------------------------------------------
# Certificate Validation Wait
# ----------------------------------------------------------------------------

resource "aws_acm_certificate_validation" "main" {
  count = var.enable_https && var.enable_route53 ? 1 : 0

  certificate_arn         = aws_acm_certificate.main[0].arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]

  timeouts {
    create = "10m"
  }
}
