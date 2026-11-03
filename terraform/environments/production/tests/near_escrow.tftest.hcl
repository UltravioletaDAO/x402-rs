# The NEAR escrow scheme (src/chain/near/escrow.rs) is on in production toward one Arras v1 escrow
# per network: v1.arras-escrow.near on mainnet, arras-v1b-1101.testnet on testnet. v2 is not
# relayed to on either until it is deployed with no key left on its account. Against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# When v2 is deployed, its contract joins the list in main.tf and the expected value below.

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

run "the_near_escrow_relays_toward_one_v1_escrow_per_network" {
  command = apply

  plan_options {
    target = [aws_ecs_task_definition.facilitator]
  }

  # On, toward v1 on each network, once each and in this order. An exact value, not a substring:
  # a v2 contract added to either list without a review of this file turns it red.
  assert {
    condition = jsonencode([
      for e in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].environment :
      e if contains(["ENABLE_NEAR_ESCROW", "NEAR_ESCROW_CONTRACTS_MAINNET", "NEAR_ESCROW_CONTRACTS_TESTNET"], e.name)
      ]) == jsonencode([
      { name = "ENABLE_NEAR_ESCROW", value = "true" },
      { name = "NEAR_ESCROW_CONTRACTS_MAINNET", value = "v1.arras-escrow.near" },
      { name = "NEAR_ESCROW_CONTRACTS_TESTNET", value = "arras-v1b-1101.testnet" },
    ])
    error_message = "The facilitator must run the NEAR escrow toward v1 alone on each network, once each: ENABLE_NEAR_ESCROW=true, NEAR_ESCROW_CONTRACTS_MAINNET=v1.arras-escrow.near and NEAR_ESCROW_CONTRACTS_TESTNET=arras-v1b-1101.testnet."
  }

  # The contract lists are public values, never secrets, and no other container sets them.
  assert {
    condition = alltrue(flatten([
      for i, c in jsondecode(aws_ecs_task_definition.facilitator.container_definitions) : [
        for name in ["ENABLE_NEAR_ESCROW", "NEAR_ESCROW_CONTRACTS_MAINNET", "NEAR_ESCROW_CONTRACTS_TESTNET"] :
        !contains([for s in try(c.secrets, []) : s.name], name) &&
        (i == 0 || !contains([for e in try(c.environment, []) : e.name], name))
      ]
    ]))
    error_message = "ENABLE_NEAR_ESCROW and the NEAR escrow contract lists are set once, in the facilitator's environment: never as a secret, never in another container."
  }
}
