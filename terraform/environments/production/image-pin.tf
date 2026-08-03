# ============================================================================
# Which image does `terraform apply` deploy?
#
# On 2026-08-03 a targeted apply — intended only to add one environment
# variable — rolled production back from 1.68.0 to 1.47.0, an image from June.
# It served live traffic on two-month-old code for about three minutes, with
# every fix from the intervening period gone, including a same-day RPC repair.
#
# Nothing was broken or unusual. The cause is structural: releases happen
# through CI, which registers a new task definition against ECS directly. CI
# never writes back to `terraform.tfvars`, so `image_tag` there ages on its own
# and silently becomes a rollback instruction. Every apply carries it. The
# person applying is usually changing something unrelated and has no reason to
# look at it.
#
# This had already been written down as a known gotcha ("verify curl /version
# against grep image_tag before applying"). It happened anyway, because a
# warning competes with attention and attention goes to whatever the operator
# came to change. So the default is inverted here instead:
#
#   Terraform now reads the image from the task definition that is ACTUALLY
#   RUNNING and redeploys the same one. An apply cannot change the version by
#   omission any more. It can only change it if someone says so out loud.
#
# To deploy a specific image on purpose:
#   terraform apply -var 'image_tag_override=1.69.0-abc1234'
#
# `image_tag` in tfvars survives only as the bootstrap value for the very first
# apply, when no task definition exists yet to read from.
# ============================================================================

data "aws_ecs_task_definition" "facilitator_current" {
  task_definition = "facilitator-production"
}

data "aws_ecs_container_definition" "facilitator_current" {
  task_definition = data.aws_ecs_task_definition.facilitator_current.id
  container_name  = "facilitator"
}

locals {
  # Precedence:
  #   1. var.image_tag when set  -> somebody asked for a version out loud.
  #      CI passes it as `-var image_tag=…` on every deploy, so releases work.
  #   2. the image currently running -> the safe no-op for a bare apply.
  #
  # The original guard here got this backwards: it treated `image_tag` as a
  # bootstrap-only fallback, which silently turned every CI deploy into a no-op.
  # The pipeline went green, ECR filled with images nobody ran, and production
  # sat on an old build. Protecting against one failure created a worse one.
  #
  # The rollback this file exists to prevent came from a STALE value sitting in
  # terraform.tfvars, not from an explicit one on the command line. So the fix
  # is to leave `image_tag` unset in tfvars: an operator running a bare apply
  # then falls through to the running image and cannot roll production back,
  # while CI, which always passes the flag, deploys exactly what it built.
  facilitator_image_registry = "${data.aws_caller_identity.current.account_id}.dkr.ecr.${data.aws_region.current.name}.amazonaws.com/${var.ecr_repository_name}"

  facilitator_image = (
    var.image_tag != ""
    ? "${local.facilitator_image_registry}:${var.image_tag}"
    : data.aws_ecs_container_definition.facilitator_current.image
  )
}

output "facilitator_image_deployed" {
  description = "The image this apply will deploy. Read it in the plan before approving."
  value       = local.facilitator_image
}
