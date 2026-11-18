# The task role's permissions boundary (main.tf, aws_iam_policy.ecs_task_boundary), and the CI
# policy's grant on the role's inline policies, which holds only under it (cicd-iam-policy.tf).
# Against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# tests/scripts/test_ci_task_role_boundary.py fails when an inline policy of the task role is
# missing from the coverage list below.

mock_provider "aws" {
  # As in bazaar_admin_token.tftest.hcl. No account id anywhere: this repo is public.
  mock_resource "aws_iam_role" {
    defaults = {
      arn = "arn:aws:iam:::role/mock"
    }
  }
  mock_resource "aws_iam_policy" {
    defaults = {
      arn = "arn:aws:iam:::policy/mock"
    }
  }
  mock_resource "aws_acm_certificate" {
    defaults = {
      arn = "arn:aws:acm:us-east-2::certificate/mock"
    }
  }
  # The task definition validates its role ARNs, and the task role's is composed from the account
  # id (local.ecs_task_role_arn): a mock's random string is not an account id. As in
  # receipts_store.tftest.hcl.
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "000000000000"
    }
  }
}
mock_provider "archive" {}

# The boundary has an ARN of its own, so the assertions below can tell it from the CI policy.
override_resource {
  target = aws_iam_policy.ecs_task_boundary
  values = { arn = "arn:aws:iam:::policy/facilitator-production-ecs-task-boundary" }
}

# Setup, first, as in nat_guard.tftest.hcl: the teardown walks the whole configuration, and the
# certificate validation records take their for_each keys from the certificates.
run "the_certificates_exist" {
  command = apply

  plan_options {
    target = [aws_acm_certificate.main, aws_acm_certificate.metrics]
  }
}

run "the_task_role_is_bounded_and_ci_writes_it_only_under_the_boundary" {
  command = apply

  plan_options {
    target = [
      aws_iam_role.ecs_task,
      aws_iam_policy.cicd_infra,
      aws_ecs_task_definition.facilitator,
      aws_iam_role_policy.dynamodb_nonce_access,
      aws_iam_role_policy.dynamodb_transactions_access,
      aws_iam_role_policy.dynamodb_idempotency_access,
      aws_iam_role_policy.s3_discovery_access,
      aws_iam_role_policy.ip_allowlist_read,
      aws_iam_role_policy.dx402_s3_access,
      aws_iam_role_policy.dx402_dynamodb_access,
      aws_iam_role_policy.hedera_settlement_access,
    ]
  }

  assert {
    condition     = aws_iam_role.ecs_task.permissions_boundary == aws_iam_policy.ecs_task_boundary.arn
    error_message = "The task role must carry aws_iam_policy.ecs_task_boundary as its permissions boundary."
  }

  # The task definition and the two inline policies the deploy applies name the role by the same
  # name and ARN AWS gives it -- composed, so the deploy does not depend on the role.
  assert {
    condition = (
      aws_ecs_task_definition.facilitator.task_role_arn == "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.ecs_task.name}" &&
      aws_iam_role_policy.hedera_settlement_access.role == aws_iam_role.ecs_task.name &&
      aws_iam_role_policy.ip_allowlist_read.role == aws_iam_role.ecs_task.name
    )
    error_message = "The task definition and the deploy's task-role policies must name facilitator-production-ecs-task by its real ARN and name."
  }

  # Every action and resource an inline policy of the role grants is inside the boundary: an
  # action by name, a resource by name or under a boundary resource ending in `*`. A policy that
  # asked for more would be cut to the boundary in production, an AccessDenied nobody planned.
  assert {
    condition = alltrue(flatten([
      for policy in [
        aws_iam_role_policy.dynamodb_nonce_access.policy,
        aws_iam_role_policy.dynamodb_transactions_access.policy,
        aws_iam_role_policy.dynamodb_idempotency_access.policy,
        aws_iam_role_policy.s3_discovery_access.policy,
        aws_iam_role_policy.ip_allowlist_read.policy,
        aws_iam_role_policy.dx402_s3_access.policy,
        aws_iam_role_policy.dx402_dynamodb_access.policy,
        aws_iam_role_policy.hedera_settlement_access.policy,
        ] : [
        for st in jsondecode(policy).Statement : [
          for action in flatten([st.Action]) : [
            for resource in flatten([st.Resource]) : anytrue([
              for b in jsondecode(aws_iam_policy.ecs_task_boundary.policy).Statement :
              contains(flatten([b.Action]), action) && anytrue([
                for br in flatten([b.Resource]) :
                endswith(br, "*") ? startswith(resource, trimsuffix(br, "*")) : br == resource
              ])
            ])
          ]
        ] if st.Effect == "Allow"
      ]
    ]))
    error_message = "An inline policy of the task role grants an action or a resource outside aws_iam_policy.ecs_task_boundary: widen the boundary in the same change."
  }

  # The boundary itself: the facilitator's stores and nothing else. No IAM, STS, SSM or other
  # service, "*" only for KMS, and KMS only through the three stores' services.
  assert {
    condition = alltrue(flatten([
      for b in jsondecode(aws_iam_policy.ecs_task_boundary.policy).Statement : [
        for action in flatten([b.Action]) :
        b.Effect == "Allow" && anytrue([for p in ["dynamodb:", "s3:", "secretsmanager:", "kms:"] : startswith(action, p)]) && !endswith(action, "*")
      ]
    ]))
    error_message = "The task role's boundary may name DynamoDB, S3, Secrets Manager and KMS actions only, each by name."
  }

  assert {
    condition = alltrue([
      for b in jsondecode(aws_iam_policy.ecs_task_boundary.policy).Statement :
      !contains(flatten([b.Resource]), "*") || (
        alltrue([for action in flatten([b.Action]) : startswith(action, "kms:")]) &&
        toset(try(b.Condition.StringEquals["kms:ViaService"], [])) == toset([
          "dynamodb.us-east-2.amazonaws.com", "s3.us-east-2.amazonaws.com", "secretsmanager.us-east-2.amazonaws.com",
        ])
      )
    ])
    error_message = "Only the boundary's KMS statement may name Resource \"*\", and only through DynamoDB, S3 and Secrets Manager."
  }

  # CI's grant to write the role's inline policies holds only while the role carries this boundary.
  assert {
    condition = alltrue(flatten([
      for st in jsondecode(aws_iam_policy.cicd_infra.policy).Statement : [
        for action in flatten([try(st.Action, [])]) :
        st.Effect != "Allow" || !anytrue([for a in ["iam:putrolepolicy", "iam:deleterolepolicy", "iam:attachrolepolicy", "iam:*", "*"] : lower(action) == a]) ||
        try(st.Condition.StringEquals["iam:PermissionsBoundary"], "") == aws_iam_policy.ecs_task_boundary.arn
      ]
    ]))
    error_message = "Every Allow of iam:PutRolePolicy or iam:DeleteRolePolicy in facilitator-cicd-infra must carry the condition iam:PermissionsBoundary = the task role's boundary."
  }

  # ...and CI can neither set, lift nor rewrite a boundary, on any role or user.
  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.cicd_infra.policy).Statement : st
      if st.Effect == "Deny" && try(st.Resource, null) == "*" && !can(st.Condition) &&
      length(setsubtract(toset([
        "iam:PutRolePermissionsBoundary", "iam:DeleteRolePermissionsBoundary",
        "iam:CreatePolicyVersion", "iam:SetDefaultPolicyVersion", "iam:DeletePolicyVersion", "iam:DeletePolicy",
      ]), toset(st.Action))) == 0
    ]) == 1
    error_message = "facilitator-cicd-infra must deny, on every resource, setting or deleting a boundary and changing a managed policy's versions."
  }

  # A managed policy holds at most 6,144 characters, whitespace not counted (jsonencode writes
  # none). The mocked account id has the real one's 12 characters; the KMS key's and the
  # boundary's ARNs are put back at their real lengths, so the policy applied by hand fits.
  assert {
    condition = length(replace(replace(aws_iam_policy.cicd_infra.policy,
      aws_kms_key.bazaar_admin_token.arn, "arn:aws:kms:us-east-2:${data.aws_caller_identity.current.account_id}:key/00000000-0000-0000-0000-000000000000"),
    aws_iam_policy.ecs_task_boundary.arn, "arn:aws:iam::${data.aws_caller_identity.current.account_id}:policy/facilitator-production-ecs-task-boundary")) <= 6144
    error_message = "facilitator-cicd-infra is over the 6,144-character limit of a managed policy."
  }
}
