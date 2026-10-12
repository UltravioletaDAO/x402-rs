# Owner's decision 230: BAZAAR_ADMIN_TOKEN is an SSM Parameter Store SecureString,
# /facilitator/bazaar-admin-token, on a KMS key of its own. Against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# Nothing here reaches AWS. The drift gate's check of the live parameter is held by
# tests/scripts/test_ci_bazaar_token.py, the CI user's deny by test_drift_gate_iam.py too.

mock_provider "aws" {
  # The task definition validates its role ARNs, and a mock makes up random strings. No account id
  # in either: this repo is public (.github/workflows/no-account-id.yml).
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

run "the_token_comes_from_ssm_on_its_own_key" {
  command = apply

  plan_options {
    target = [
      aws_ecs_task_definition.facilitator,
      aws_iam_role_policy.secrets_access,
      aws_iam_policy.cicd_infra,
      aws_kms_alias.bazaar_admin_token,
    ]
  }

  # The task definition reads it from SSM, by the parameter's ARN -- not from Secrets Manager.
  assert {
    condition = [
      for s in jsondecode(aws_ecs_task_definition.facilitator.container_definitions)[0].secrets :
      s.valueFrom if s.name == "BAZAAR_ADMIN_TOKEN"
    ] == ["arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter/facilitator/bazaar-admin-token"]
    error_message = "BAZAAR_ADMIN_TOKEN must come once, from the SSM parameter /facilitator/bazaar-admin-token, by its ARN."
  }

  # Its own key, rotated every year, under the alias the parameter is put with.
  assert {
    condition     = aws_kms_key.bazaar_admin_token.enable_key_rotation == true && aws_kms_key.bazaar_admin_token.rotation_period_in_days == 365
    error_message = "The token's KMS key must rotate every year (enable_key_rotation, rotation_period_in_days = 365)."
  }

  assert {
    condition     = aws_kms_alias.bazaar_admin_token.name == "alias/facilitator-bazaar-admin-token" && aws_kms_alias.bazaar_admin_token.target_key_id == aws_kms_key.bazaar_admin_token.key_id
    error_message = "alias/facilitator-bazaar-admin-token must name the token's key."
  }

  # The execution role reads that one parameter...
  assert {
    condition = length([
      for st in jsondecode(aws_iam_role_policy.secrets_access.policy).Statement : st
      if try(st.Effect, "") == "Allow" &&
      try(st.Action, null) == ["ssm:GetParameters"] &&
      try(st.Resource, null) == "arn:aws:ssm:us-east-2:${data.aws_caller_identity.current.account_id}:parameter/facilitator/bazaar-admin-token"
    ]) == 1
    error_message = "The execution role must be granted ssm:GetParameters on /facilitator/bazaar-admin-token, and only there."
  }

  # ...and decrypts with that one key, only through SSM.
  assert {
    condition = length([
      for st in jsondecode(aws_iam_role_policy.secrets_access.policy).Statement : st
      if try(st.Effect, "") == "Allow" &&
      try(st.Action, null) == ["kms:Decrypt"] &&
      try(st.Condition.StringEquals["kms:ViaService"], null) == "ssm.us-east-2.amazonaws.com" &&
      try(st.Condition["ForAnyValue:StringEquals"]["kms:ResourceAliases"], null) == "alias/facilitator-bazaar-admin-token"
    ]) == 1
    error_message = "The execution role must be granted kms:Decrypt on the token's key (by its alias) through SSM alone."
  }

  assert {
    condition = alltrue([
      for st in jsondecode(aws_iam_role_policy.secrets_access.policy).Statement :
      !contains(try(st.Action, []), "kms:Decrypt") || can(st.Condition.StringEquals["kms:ViaService"])
    ])
    error_message = "No kms:Decrypt grant on the execution role may stand without the SSM-only condition."
  }

  # The CI user cannot decrypt with it, whatever else is attached.
  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.cicd_infra.policy).Statement : st
      if try(st.Sid, "") == "DenyBazaarTokenDecrypt" &&
      try(st.Effect, "") == "Deny" &&
      try(st.Action, null) == ["kms:Decrypt"] &&
      try(st.Resource, null) == aws_kms_key.bazaar_admin_token.arn &&
      !can(st.Condition)
    ]) == 1
    error_message = "aws_iam_policy.cicd_infra must deny kms:Decrypt on the token's key, unconditionally."
  }
}
