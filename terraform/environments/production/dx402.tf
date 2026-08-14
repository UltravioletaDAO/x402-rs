# ============================================================================
# DX402 -- durable-evidence extension
# ============================================================================
#
# x402 settles payment on-chain permanently but delivers the purchased resource
# exactly once and keeps nothing. DX402 seals a copy of the response to the
# payer's own public key -- recovered from the payment signature itself -- and
# anchors it here.
#
# Two stores, and they hold very different things:
#
#   S3       the sealed CIPHERTEXT. Unreadable without the payer's private key.
#   DynamoDB the INDEX: paymentId -> pointer, content hash, signed receipt.
#            No key material in `direct` mode, which is the default.
#
# A leak of either reveals pointers and hashes, never payloads.
#
# Docs: docs/DX402.md   Spec: docs/plans/dx402/02-SPEC-v0.1.md
#
# ----------------------------------------------------------------------------
# Cost when idle: effectively zero. DynamoDB is PAY_PER_REQUEST and an empty
# bucket costs nothing, so these are created unconditionally. The FEATURE is
# gated separately by var.enable_dx402, which controls the environment
# variables -- see main.tf. Provision first, flip the switch second.
# ----------------------------------------------------------------------------

# ----------------------------------------------------------------------------
# S3 -- sealed evidence
# ----------------------------------------------------------------------------

resource "aws_s3_bucket" "dx402_evidence" {
  bucket = "facilitator-dx402-evidence-${var.environment}"

  tags = {
    Name        = "facilitator-dx402-evidence"
    Environment = var.environment
    Purpose     = "DX402 sealed response bodies (ciphertext only)"
  }
}

# The bucket is PRIVATE and stays private.
#
# Buyers never read S3 directly: a DX402 pointer resolves through the
# facilitator's own `GET /dx402/blob/{paymentId}`, which streams the ciphertext
# back. That costs one hop and removes a publicly-readable bucket from the
# design entirely -- the single most common way object storage leaks.
#
# It also decouples the pointer from the key layout, so pointers a buyer is
# holding a year from now keep resolving even if these keys are reorganised.
resource "aws_s3_bucket_public_access_block" "dx402_evidence" {
  bucket = aws_s3_bucket.dx402_evidence.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "dx402_evidence" {
  bucket = aws_s3_bucket.dx402_evidence.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

# Encryption at rest, on top of the DX402 envelope.
#
# Belt and braces: the payload is already AES-256-GCM sealed to the payer before
# it ever reaches S3, so SSE protects nothing an attacker could use. It is here
# so that "is the bucket encrypted" has a boring answer in any audit.
resource "aws_s3_bucket_server_side_encryption_configuration" "dx402_evidence" {
  bucket = aws_s3_bucket.dx402_evidence.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

# Versioning stays OFF deliberately.
#
# Retention is a promise DX402 makes to the buyer, and versioning would keep
# "deleted" objects as noncurrent versions past the window we committed to.
# A retention policy that does not actually delete is not a retention policy.
resource "aws_s3_bucket_versioning" "dx402_evidence" {
  bucket = aws_s3_bucket.dx402_evidence.id

  versioning_configuration {
    status = "Disabled"
  }
}

# Retention, enforced by lifecycle rules keyed on the tag the facilitator writes
# at upload time (`dx402-retention=90d|1y|permanent`, see src/dx402/store.rs).
#
# Keeping the policy on the bucket rather than as per-object expiry dates keeps
# it in one auditable place instead of scattered across millions of objects.
#
# The windows here are one day LONGER than the API's own expiry. The index
# returns 410 Gone at exactly retentionUntil, so the object is already
# unreachable before S3 removes it -- there is never a moment where the index
# says evidence exists and the bytes are gone.
resource "aws_s3_bucket_lifecycle_configuration" "dx402_evidence" {
  bucket = aws_s3_bucket.dx402_evidence.id

  # Default retention. Bounded on purpose: anchoring is publishing, and an
  # unbounded default would make one careless anchor of personal data
  # unfixable.
  rule {
    id     = "dx402-retention-90d"
    status = "Enabled"

    filter {
      tag {
        key   = "dx402-retention"
        value = "90d"
      }
    }

    expiration {
      days = 91
    }
  }

  rule {
    id     = "dx402-retention-1y"
    status = "Enabled"

    filter {
      tag {
        key   = "dx402-retention"
        value = "1y"
      }
    }

    expiration {
      days = 366
    }
  }

  # `permanent` gets NO expiration rule -- that is the whole point of it, and it
  # is irrevocable. Nothing here can undo an anchor a seller marked permanent.

  # Housekeeping: a failed multipart upload otherwise bills forever.
  rule {
    id     = "abort-incomplete-uploads"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# ----------------------------------------------------------------------------
# DynamoDB -- the evidence index
# ----------------------------------------------------------------------------
#
# A LOOKUP, not a ledger. The authoritative artifacts are the sealed blob in S3
# and the signed receipt the buyer already holds; both stay verifiable if this
# table is lost entirely. Same discipline as the transactions table: the chain is
# the ledger, and nothing here gates a payment.
#
# What it buys is the case where a buyer returns months later with nothing but a
# transaction hash and asks "what did I actually buy?".
resource "aws_dynamodb_table" "dx402_evidence" {
  name         = "facilitator_dx402_evidence"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "payment_id"

  attribute {
    name = "payment_id"
    type = "S"
  }

  # Written from the record's retention_until, so the index expires in step with
  # the promise. `permanent` records carry no expires_at and are never removed.
  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  # Evidence exists to settle disputes. Losing the index to an operator mistake
  # would mean losing the ability to answer "what did I buy?" for every payment
  # at once.
  point_in_time_recovery {
    enabled = true
  }

  tags = {
    Name        = "facilitator-dx402-evidence"
    Environment = var.environment
    Purpose     = "DX402 paymentId -> pointer index (no key material)"
  }
}

# ----------------------------------------------------------------------------
# IAM -- task role permissions
# ----------------------------------------------------------------------------

resource "aws_iam_role_policy" "dx402_s3_access" {
  name = "DX402EvidenceS3Access"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          # The facilitator tags each object with its retention window; the
          # lifecycle rules above are driven entirely by that tag, so without
          # this permission nothing would ever expire.
          "s3:PutObjectTagging"
        ]
        # No DeleteObject on purpose. Expiry is the lifecycle policy's job, and
        # the running task has no business deleting evidence someone may be
        # about to cite in a dispute.
        Resource = "${aws_s3_bucket.dx402_evidence.arn}/*"
      },
      {
        Effect   = "Allow"
        Action   = ["s3:ListBucket"]
        Resource = aws_s3_bucket.dx402_evidence.arn
      }
    ]
  })
}

resource "aws_iam_role_policy" "dx402_dynamodb_access" {
  name = "DX402EvidenceDynamoDBAccess"
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
          # Scan backs the anchor counter on the landing page and /dx402/stats.
          # Acceptable at our volume; if this table ever grows large the counter
          # should move to an atomic counter item rather than a full scan on a
          # public route.
          "dynamodb:Scan"
        ]
        # No DeleteItem: expiry is DynamoDB's job via TTL.
        Resource = aws_dynamodb_table.dx402_evidence.arn
      }
    ]
  })
}

# ----------------------------------------------------------------------------
# Outputs
# ----------------------------------------------------------------------------

output "dx402_evidence_bucket" {
  description = "S3 bucket holding DX402 sealed evidence (ciphertext only)"
  value       = aws_s3_bucket.dx402_evidence.id
}

output "dx402_evidence_table" {
  description = "DynamoDB table indexing DX402 evidence by paymentId"
  value       = aws_dynamodb_table.dx402_evidence.name
}

output "dx402_enabled" {
  description = "Whether the facilitator is configured to produce DX402 evidence"
  value       = var.enable_dx402
}
