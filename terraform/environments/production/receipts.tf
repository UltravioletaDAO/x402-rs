# Dedicated Ed25519 receipt key. Provision the secret out of band before the
# first deployment; Terraform never reads its private value into state.
data "aws_secretsmanager_secret" "facilitator_receipt_key" {
  name = "facilitator-receipt-signing-key"
}

locals {
  # The SSM copy of the secret's private_key field (secrets.tf, decision 266).
  receipt_secrets = [{
    name      = "RECEIPT_SIGNING_KEY"
    valueFrom = "${local.facilitator_parameter_arn_prefix}/receipt-signing-key/private_key"
  }]
}
