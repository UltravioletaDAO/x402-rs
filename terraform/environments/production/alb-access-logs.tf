# ============================================================================
# ALB access logs -- S3 bucket, decoupled from the routine deploy graph
# ============================================================================
#
# The 2026-08 performance diagnosis found 201 of 417 real failures were HTTP
# 460 (ALB-side idle-timeout-during-request), a code that is INVISIBLE without
# access logs -- it never reaches the application, so it is not in our own
# CloudWatch logs and not in any /events or /transactions record. This bucket
# is where that becomes visible.
#
# ----------------------------------------------------------------------------
# Why this bucket is NOT wired into aws_lb.main yet by default
# ----------------------------------------------------------------------------
#
# Bucket creation happens here, unconditionally, decoupled from aws_lb.main by
# a plain string (local.alb_access_logs_bucket_name) rather than a resource
# reference -- the same reason dx402.tf's bucket is named instead of
# referenced: aws_lb.main is already inside the dependency graph CI's routine
# deploy touches (-target=aws_ecs_task_definition.facilitator
# -target=aws_ecs_service.facilitator pulls it in via the listener depends_on
# chain), so ANY resource this bucket's creation depended on would get dragged
# into every future deploy, and CI has no S3 permissions.
#
# What decoupling can NOT protect against: aws_lb.main itself is already in
# that graph regardless of what this file does. The access_logs block is
# gated behind var.alb_access_logs_enabled (main.tf) specifically so that
# creating this bucket and wiring the ALB to use it are two separate applies --
# see that variable's description for the exact sequencing. Do not add a
# `depends_on` from aws_lb.main to anything in this file; that would reproduce
# the same problem the string-literal decoupling exists to avoid.

locals {
  alb_access_logs_bucket_name = "facilitator-${var.environment}-alb-logs"
}

resource "aws_s3_bucket" "alb_logs" {
  bucket = local.alb_access_logs_bucket_name

  tags = {
    Name        = "facilitator-${var.environment}-alb-logs"
    Environment = var.environment
    Purpose     = "ALB access logs -- makes ALB-side failures (460, etc.) visible"
  }
}

resource "aws_s3_bucket_public_access_block" "alb_logs" {
  bucket = aws_s3_bucket.alb_logs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "alb_logs" {
  bucket = aws_s3_bucket.alb_logs.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "alb_logs" {
  bucket = aws_s3_bucket.alb_logs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "alb_logs" {
  bucket = aws_s3_bucket.alb_logs.id

  rule {
    id     = "expire-after-retention"
    status = "Enabled"

    filter {}

    expiration {
      days = var.alb_access_logs_retention_days
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# The account used to deliver ALB access logs is region-specific and NOT the
# same as our own account -- this data source resolves it instead of
# hardcoding a magic account ID that would silently be wrong in another
# region. Required in every region regardless of launch date; the newer
# `delivery.logs.amazonaws.com` statements below are additionally required and
# recommended for all regions per AWS's current guidance.
data "aws_elb_service_account" "main" {}

resource "aws_s3_bucket_policy" "alb_logs" {
  bucket = aws_s3_bucket.alb_logs.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "ELBAccountWrite"
        Effect = "Allow"
        Principal = {
          AWS = data.aws_elb_service_account.main.arn
        }
        Action   = "s3:PutObject"
        Resource = "${aws_s3_bucket.alb_logs.arn}/alb/AWSLogs/${data.aws_caller_identity.current.account_id}/*"
      },
      {
        Sid    = "LogDeliveryWrite"
        Effect = "Allow"
        Principal = {
          Service = "delivery.logs.amazonaws.com"
        }
        Action   = "s3:PutObject"
        Resource = "${aws_s3_bucket.alb_logs.arn}/alb/AWSLogs/${data.aws_caller_identity.current.account_id}/*"
        Condition = {
          StringEquals = {
            "s3:x-amz-acl"      = "bucket-owner-full-control"
            "aws:SourceAccount" = data.aws_caller_identity.current.account_id
          }
        }
      },
      {
        Sid    = "LogDeliveryAclCheck"
        Effect = "Allow"
        Principal = {
          Service = "delivery.logs.amazonaws.com"
        }
        Action   = "s3:GetBucketAcl"
        Resource = aws_s3_bucket.alb_logs.arn
        Condition = {
          StringEquals = {
            "aws:SourceAccount" = data.aws_caller_identity.current.account_id
          }
        }
      }
    ]
  })
}
