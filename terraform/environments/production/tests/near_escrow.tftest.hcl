# The NEAR escrow scheme (src/chain/near/escrow.rs) is on in production toward the testnet escrow
# only. NEAR mainnet has no escrow deployed yet: the task definition names no mainnet contract, so
# nothing is relayed there. Against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# When the mainnet escrow is deployed, its contract goes into main.tf and the mainnet assertion
# below goes with it.

mock_provider "aws" {
  # As in bazaar_admin_token.tftest.hcl: the task definition validates its role ARNs, and a mock
  # makes up random strings. No account id in either: this repo is public.
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
}
mock_provider "archive" {}

# Setup, first, as in nat_guard.tftest.hcl: the teardown walks the whole configuration, and the
# certificate validation records take their for_each keys from the certificates.
run "the_certificates_exist" {
  command = apply

  plan_options {
    target = [aws_acm_certificate.main, aws_acm_certificate.metrics]
  }
}

run "the_near_escrow_relays_toward_the_testnet_escrow_only" {
  command = apply

  plan_options {
    target = [aws_ecs_task_definition.facilitator]
  }

  # On, toward the testnet escrow. It also shows the next assertion reads the facilitator's own
  # environment, and not a list where no NEAR escrow variable could ever be.
  assert {
    condition = jsonencode([
      for e in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].environment :
      e if contains(["ENABLE_NEAR_ESCROW", "NEAR_ESCROW_CONTRACTS_TESTNET"], e.name)
      ]) == jsonencode([
      { name = "ENABLE_NEAR_ESCROW", value = "true" },
      { name = "NEAR_ESCROW_CONTRACTS_TESTNET", value = "arras-v1b-1101.testnet" },
    ])
    error_message = "The facilitator must run the NEAR escrow toward the testnet escrow, once each: ENABLE_NEAR_ESCROW=true and NEAR_ESCROW_CONTRACTS_TESTNET=arras-v1b-1101.testnet."
  }

  # No mainnet contract, in any container, as a value or as a secret.
  assert {
    condition = alltrue([
      for c in jsondecode(aws_ecs_task_definition.facilitator.container_definitions) :
      !contains([for e in try(c.environment, []) : e.name], "NEAR_ESCROW_CONTRACTS_MAINNET") &&
      !contains([for s in try(c.secrets, []) : s.name], "NEAR_ESCROW_CONTRACTS_MAINNET")
    ])
    error_message = "NEAR mainnet has no escrow deployed: no container may set NEAR_ESCROW_CONTRACTS_MAINNET until it is (and then this assertion goes)."
  }
}
