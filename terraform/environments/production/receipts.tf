# Dedicated Ed25519 receipt key. Provision the secret out of band before the
# first deployment; Terraform never reads its private value into state.
data "aws_secretsmanager_secret" "facilitator_receipt_key" {
  name = "facilitator-receipt-signing-key"
}

locals {
  receipt_secrets = [{
    name      = "RECEIPT_SIGNING_KEY"
    valueFrom = "${data.aws_secretsmanager_secret.facilitator_receipt_key.arn}:private_key::"
  }]
}
