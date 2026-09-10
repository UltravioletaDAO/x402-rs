# ============================================================================
# The Bazaar catalog bucket -- versioning and version retention
# ============================================================================
#
# `facilitator-discovery-prod` holds the discovery catalog. It predates this
# Terraform configuration and was created by hand, so only its CONFIGURATION is
# managed here, never the bucket itself. See "The bucket is not declared" below.
#
# ----------------------------------------------------------------------------
# Why versioning, when the write path is already conditional
# ----------------------------------------------------------------------------
#
# Every registration is a read-modify-write of ONE object: GET the whole
# catalog, edit it in memory, PUT the whole catalog back. `bazaar/resources.json`
# was 15,206,413 bytes on 2026-09-10, so each edit rewrites ~15 MB and the
# previous catalog stops existing at that instant.
#
# PR #31 made those writes conditional on the ETag the reader saw
# (`src/discovery_store.rs`, If-Match / If-None-Match), which is the real
# mechanism: two concurrent writers no longer silently overwrite each other,
# one of them is refused. That is a correctness fix and it stays the mechanism.
#
# What it does NOT give is a way back. A write that is correct by every check
# and still wrong -- a bad merge, a bad migration, a curation pass that drops
# rows it should have kept -- lands as the one and only copy. The audit of
# 2026-09-09 (section 6, A3) asked for versioning as the ESCAPE HATCH behind
# that mechanism, not as a replacement for it.
#
# With versioning on, the pre-write catalog is still addressable by version id
# and recovery is a copy, not an archaeology project.
#
# ----------------------------------------------------------------------------
# The bucket is not declared here, and that is deliberate
# ----------------------------------------------------------------------------
#
# `aws_s3_bucket_versioning` and `aws_s3_bucket_lifecycle_configuration` take a
# bucket NAME, so they manage the bucket's configuration without Terraform
# owning the bucket. Two things follow, both of them wanted:
#
#   - No `terraform import` is needed to land this, and no plan can ever propose
#     destroying or replacing a bucket that holds the live catalog.
#   - Naming the bucket as a literal string rather than as
#     `aws_s3_bucket.discovery.id` records no dependency edge, so a targeted
#     apply of these two resources drags nothing else in. Same reasoning as the
#     `locals` block at the top of dx402.tf, which was written after CI run
#     32063044613 failed on `AccessDenied ... s3:CreateBucket`.
#
# If the bucket should later be managed here too, adopt it without recreating it:
#
#   terraform import aws_s3_bucket.discovery facilitator-discovery-prod
#
# Do not run that as part of shipping this file -- an import is a state edit and
# belongs in its own reviewed change.
#
# ----------------------------------------------------------------------------
# IAM: the deploy user needs two permissions it does not have yet
# ----------------------------------------------------------------------------
#
# Simulated against the live identity on 2026-09-10, not assumed:
#
#   s3:PutBucketVersioning        implicitDeny
#   s3:PutLifecycleConfiguration  implicitDeny
#   s3:GetBucketVersioning        allowed    (via the attached ReadOnlyAccess)
#   s3:GetLifecycleConfiguration  allowed    (via the attached ReadOnlyAccess)
#
# The reads are what the drift gate's plan needs, so the gate works today. The
# writes are what the deploy step needs, and until they are granted the apply
# fails with AccessDenied.
#
# The grant is declared alongside every other one, as the `DiscoveryBucketVersioning`
# statement in cicd-iam-policy.tf -- scoped to this one bucket, and to its
# CONFIGURATION only: simulated before it was asked for, it allows no object read,
# no object write and no version delete, on this bucket or any other.
#
# It is applied BY HAND, before this merges, because aws_iam_policy.cicd_infra is
# deliberately outside every deploy target list: if CI could apply it, CI could grant
# itself permissions. So the drift gate flags that one resource until a human runs
# the apply, which is the gate working rather than failing. Sequence and command:
# docs/handoffs/2026-09-10-bucket-versioning.md.
# ----------------------------------------------------------------------------

resource "aws_s3_bucket_versioning" "discovery" {
  bucket = "facilitator-discovery-prod"

  versioning_configuration {
    status = "Enabled"
  }
}

# ----------------------------------------------------------------------------
# Version retention
# ----------------------------------------------------------------------------
#
# Versioning without an expiry is a bill that only grows, and this bucket is
# rewritten constantly. Measured 2026-09-10, both keys, in the same bucket:
#
#   bazaar/resources.json   14.5 MiB   rewritten on aggregation and on each
#                                      registration. Last write was ~3h old at
#                                      the time of measurement, so the real rate
#                                      is single-digit to low-tens per day.
#   bazaar/health.json       6.4 MiB   rewritten by the health prober's persist,
#                                      once per 60s tick whenever the overlay is
#                                      dirty (`src/discovery_health.rs`). Two
#                                      independent reads four minutes apart each
#                                      found it written seconds earlier, so treat
#                                      1440/day as the working figure.
#
# At a flat 30-day retention that second key alone would hold ~43,200 noncurrent
# versions, ~271 GB, ~$6.20/month -- roughly twenty times what the catalog it is
# meant to protect would cost. And it would buy nothing: the health overlay is
# DERIVED state. Every record in it is rebuilt by probing, so an old version of
# it is not a recovery point, it is a stale copy of something the prober will
# regenerate within the hour.
#
# So: 30 days for the bucket, one day for the overlay. Both rules match
# `bazaar/health.json`, and where two expiration rules cover the same object S3
# applies the shorter one.
resource "aws_s3_bucket_lifecycle_configuration" "discovery" {
  bucket = "facilitator-discovery-prod"

  # An expiry rule on noncurrent versions is meaningless until versioning is on,
  # and on a first apply Terraform has no reason of its own to order the two.
  depends_on = [aws_s3_bucket_versioning.discovery]

  # The escape hatch itself: a month of catalogs to fall back to.
  #
  # 30 days is chosen against how the failure is FOUND, not how it happens. A
  # bad write is noticed when somebody misses their listing, which is days after
  # the fact, not seconds.
  rule {
    id     = "noncurrent-versions-30d"
    status = "Enabled"

    filter {}

    noncurrent_version_expiration {
      noncurrent_days = 30
    }
  }

  # The health overlay, carved out. See the measurement above.
  rule {
    id     = "health-overlay-noncurrent-1d"
    status = "Enabled"

    filter {
      prefix = "bazaar/health.json"
    }

    noncurrent_version_expiration {
      noncurrent_days = 1
    }
  }

  # Housekeeping: a failed multipart upload otherwise bills forever. Same rule,
  # same reason, as the one on the DX402 bucket in dx402.tf.
  rule {
    id     = "abort-incomplete-uploads"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}
