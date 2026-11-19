# Payment paths production keeps switched off, held to their exact values so a merge that drops or
# flips one of them is red instead of shipping it on. Against a mocked AWS.
#
# ENABLE_SETTLEMENT_ACCOUNT: the Solana/Fogo settlement-account path (src/chain/solana.rs) is ON
# unless this variable says "false" or "0", so losing the entry turns the path back on. It must be
# present exactly once, in the facilitator's environment, with the value "false".
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test

mock_provider "aws" {
  # As in near_escrow.tftest.hcl: the task definition validates its role ARNs, and a mock makes up
  # random strings. No account id: this repo is public.
  mock_resource "aws_iam_role" {
    defaults = {
      arn = "arn:aws:iam:::role/mock"
    }
  }
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-2::certificate/mock"
    }
  }
  # The task role's ARN is composed from the account id (local.ecs_task_role_arn): a mock's random
  # string is not an account id. As in ecs_task_boundary.tftest.hcl.
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "000000000000"
    }
  }
}
mock_provider "archive" {}

run "the_certificates_exist" {
  command = apply

  plan_options {
    target = [aws_acm_certificate.main, aws_acm_certificate.metrics]
  }
}

run "the_settlement_account_path_stays_off" {
  command = apply

  plan_options {
    target = [aws_ecs_task_definition.facilitator]
  }

  assert {
    condition = jsonencode([
      for e in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].environment :
      e if e.name == "ENABLE_SETTLEMENT_ACCOUNT"
    ]) == jsonencode([{ name = "ENABLE_SETTLEMENT_ACCOUNT", value = "false" }])
    error_message = "The facilitator must run with ENABLE_SETTLEMENT_ACCOUNT=false, once: without it the Solana/Fogo settlement-account path is on."
  }

  assert {
    condition = alltrue([
      for c in jsondecode(aws_ecs_task_definition.facilitator.container_definitions) :
      !contains([for s in try(c.secrets, []) : s.name], "ENABLE_SETTLEMENT_ACCOUNT")
    ])
    error_message = "ENABLE_SETTLEMENT_ACCOUNT is a public value in the environment, never a secret."
  }
}
