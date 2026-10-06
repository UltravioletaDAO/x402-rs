# ============================================================================
# Lifecycle of the ECR `facilitator` repository (COSTO-X402 B15)
# ============================================================================
#
# The repository itself is not managed here (it predates this Terraform); only its
# lifecycle policy is. An expiry cannot be undone, so the policy ships OFF and is turned
# on by hand, in this order, by someone with ECR/ECS read and ecr:PutImage:
#
#   1. python3 scripts/ecr_rollback_anchors.py --tag      # stable-* on every anchor
#   2. python3 scripts/ecr_rollback_anchors.py --preview  # must exit 0
#   3. enable_facilitator_ecr_lifecycle = true (variables.tf + production.auto.tfvars), apply
#
# Anchors are the running and in-flight images, the last ACTIVE revisions of the task
# definition family and the tags the repo names as rollback targets; rule 1 never
# expires a stable-* image. The preview also stops if a kept index would lose a child
# manifest (CI's buildx pushes indexes with untagged children). Turning the flag back
# off deletes the policy and stops further expiry; it does not bring an image back.
# Not in any CI -target list: only a full apply touches it.

resource "aws_ecr_lifecycle_policy" "facilitator" {
  count      = var.enable_facilitator_ecr_lifecycle ? 1 : 0
  repository = var.ecr_repository_name
  policy     = file("${path.module}/ecr-facilitator-lifecycle.json")
}
