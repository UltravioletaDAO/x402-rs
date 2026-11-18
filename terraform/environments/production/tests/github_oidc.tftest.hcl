# The CI's AWS identities by GitHub OIDC (cicd-oidc.tf), against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# What ci.yaml asks of these roles (which job assumes which, the environment, id-token) is held by
# tests/scripts/test_ci_oidc.py.

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
  # The attachments validate the policy ARN, and the teardown validates every alarm's SNS action:
  # ARN-shaped defaults for what the CI policies reference, without an account.
  mock_resource "aws_iam_policy" {
    defaults = {
      arn = "arn:aws:iam:::policy/mock"
    }
  }
  mock_resource "aws_sns_topic" {
    defaults = {
      arn = "arn:aws:sns:us-east-2::facilitator-production-alerts"
    }
  }
  mock_resource "aws_sqs_queue" {
    defaults = {
      arn = "arn:aws:sqs:us-east-2::facilitator-production-alerts-backup"
    }
  }
  mock_resource "aws_lambda_function" {
    defaults = {
      arn = "arn:aws:lambda:us-east-2::function:facilitator-production-balances"
    }
  }
  mock_resource "aws_kms_key" {
    defaults = {
      arn = "arn:aws:kms:us-east-2::key/00000000-0000-0000-0000-000000000000"
    }
  }
  mock_resource "aws_s3_bucket" {
    defaults = {
      arn = "arn:aws:s3:::facilitator-production-mock"
    }
  }
  # A fixed, obviously fake account: the teardown validates ARNs other stacks' alarms compose from it.
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "000000000000"
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

run "the_ci_roles_trust_github_narrowly_and_grant_only_what_a_release_needs" {
  command = apply

  plan_options {
    target = [
      aws_iam_role_policy_attachment.github_plan_read,
      aws_iam_role_policy_attachment.github_deploy_read,
      aws_iam_role_policy_attachment.github_deploy_writes,
      aws_iam_role_policy_attachment.github_deploy_infra,
    ]
  }

  # Distinct ARNs for the three roles a release passes, so the PassRole checks below can tell
  # them apart (the mock gives every role the same one).
  override_resource {
    target = aws_iam_role.ecs_task_execution
    values = { arn = "arn:aws:iam:::role/facilitator-production-ecs-execution" }
  }
  override_resource {
    target = aws_iam_role.ecs_task
    values = { arn = "arn:aws:iam:::role/facilitator-production-ecs-task" }
  }
  override_resource {
    target = aws_iam_role.balances_lambda
    values = { arn = "arn:aws:iam:::role/facilitator-production-balances-lambda" }
  }

  # --- Trust: one statement each, GitHub's provider, the STS audience, an exact subject list.

  assert {
    condition = alltrue([
      for doc in [aws_iam_role.github_plan.assume_role_policy, aws_iam_role.github_deploy.assume_role_policy] :
      length(jsondecode(doc).Statement) == 1 &&
      jsondecode(doc).Statement[0].Effect == "Allow" &&
      jsondecode(doc).Statement[0].Action == "sts:AssumeRoleWithWebIdentity" &&
      jsondecode(doc).Statement[0].Principal.Federated == "arn:aws:iam::${data.aws_caller_identity.current.account_id}:oidc-provider/token.actions.githubusercontent.com" &&
      toset(keys(jsondecode(doc).Statement[0].Condition)) == toset(["StringEquals"]) &&
      toset(keys(jsondecode(doc).Statement[0].Condition.StringEquals)) == toset(["token.actions.githubusercontent.com:aud", "token.actions.githubusercontent.com:sub"]) &&
      jsondecode(doc).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:aud"] == "sts.amazonaws.com"
    ])
    error_message = "Each CI role trusts GitHub's OIDC provider alone, for audience sts.amazonaws.com, with StringEquals on aud and sub only (no StringLike, no wildcard)."
  }

  assert {
    condition = toset(jsondecode(aws_iam_role.github_deploy.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]) == toset(["repo:UltravioletaDAO/x402-rs:environment:production"])
    error_message = "facilitator-github-deploy must trust the production environment of UltravioletaDAO/x402-rs and nothing else."
  }

  assert {
    condition = toset(jsondecode(aws_iam_role.github_plan.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]) == toset([
      "repo:UltravioletaDAO/x402-rs:pull_request",
      "repo:UltravioletaDAO/x402-rs:ref:refs/heads/main",
    ])
    error_message = "facilitator-github-plan must trust pull requests and main's ref of UltravioletaDAO/x402-rs, and nothing else."
  }

  # --- Attachments: the plan role reads; the deploy role reads, writes the release, and keeps cicd_infra.

  assert {
    condition = (
      aws_iam_role_policy_attachment.github_plan_read.role == aws_iam_role.github_plan.name &&
      aws_iam_role_policy_attachment.github_plan_read.policy_arn == aws_iam_policy.github_read.arn &&
      aws_iam_role_policy_attachment.github_deploy_read.role == aws_iam_role.github_deploy.name &&
      aws_iam_role_policy_attachment.github_deploy_read.policy_arn == aws_iam_policy.github_read.arn &&
      aws_iam_role_policy_attachment.github_deploy_writes.role == aws_iam_role.github_deploy.name &&
      aws_iam_role_policy_attachment.github_deploy_writes.policy_arn == aws_iam_policy.github_deploy_writes.arn &&
      aws_iam_role_policy_attachment.github_deploy_infra.role == aws_iam_role.github_deploy.name &&
      aws_iam_role_policy_attachment.github_deploy_infra.policy_arn == aws_iam_policy.cicd_infra.arn
    )
    error_message = "The plan role holds github_read; the deploy role holds github_read, github_deploy_writes and cicd_infra."
  }

  # --- PassRole: the three roles of a release, each only to the service that runs it.

  assert {
    condition = alltrue([
      for st in jsondecode(aws_iam_policy.github_deploy_writes.policy).Statement :
      # try(): in 1.9.8 || does not short-circuit, so a statement without Condition must not error here.
      !contains(flatten([st.Action]), "iam:PassRole") || (
        st.Effect == "Allow" &&
        length(flatten([st.Action])) == 1 &&
        try(toset(keys(st.Condition)), toset([])) == toset(["StringEquals"]) &&
        try(toset(keys(st.Condition.StringEquals)), toset([])) == toset(["iam:PassedToService"])
      )
    ])
    error_message = "Every iam:PassRole grant stands alone in its statement, conditioned on iam:PassedToService."
  }

  assert {
    condition = toset(flatten([
      for st in jsondecode(aws_iam_policy.github_deploy_writes.policy).Statement : [
        for r in flatten([st.Resource]) : "${r} -> ${st.Condition.StringEquals["iam:PassedToService"]}"
      ] if contains(flatten([st.Action]), "iam:PassRole")
      ])) == toset([
      "arn:aws:iam:::role/facilitator-production-ecs-execution -> ecs-tasks.amazonaws.com",
      "arn:aws:iam:::role/facilitator-production-ecs-task -> ecs-tasks.amazonaws.com",
      "arn:aws:iam:::role/facilitator-production-balances-lambda -> lambda.amazonaws.com",
    ])
    error_message = "PassRole reaches exactly the execution and task roles (to ECS tasks) and the balances Lambda role (to Lambda)."
  }

  # --- No wildcard action, no NotAction/NotResource in an Allow, and "*" resources only where IAM leaves no choice.

  assert {
    condition = alltrue(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement :
        st.Effect != "Allow" || (
          !can(st.NotAction) && !can(st.NotResource) &&
          alltrue([for a in flatten([st.Action]) : !strcontains(a, "*")])
        )
      ]
    ]))
    error_message = "An Allow in the CI policies names its actions one by one: no wildcard, no NotAction, no NotResource."
  }

  assert {
    condition = alltrue(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement : [
          for a in flatten([st.Action]) :
          can(regex("^[a-z0-9-]+:(Describe|List)", a)) || contains([
            "route53:GetHostedZone", "ecr:GetAuthorizationToken", "ecs:DeregisterTaskDefinition",
          ], a)
        ] if st.Effect == "Allow" && contains(flatten([st.Resource]), "*")
      ]
    ]))
    error_message = "Resource \"*\" is allowed only for Describe/List calls, ECR login and DeregisterTaskDefinition, which IAM cannot scope."
  }

  # --- Reads that return data: none, except the state, its lock digest and the balances Lambda.

  assert {
    condition = alltrue(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement : [
          for a in flatten([st.Action]) : !contains([
            "secretsmanager:GetSecretValue", "ssm:GetParameter", "ssm:GetParameters", "ssm:GetParametersByPath",
            "ssm:GetParameterHistory", "kms:Decrypt", "s3:GetObjectVersion", "dynamodb:Query", "dynamodb:Scan",
            "dynamodb:BatchGetItem", "logs:GetLogEvents", "logs:FilterLogEvents", "logs:StartQuery",
          ], a)
        ] if st.Effect == "Allow"
      ]
    ]))
    error_message = "No CI policy allows reading a secret, a parameter, a decrypted value, DynamoDB items or log events."
  }

  assert {
    condition = alltrue(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement : [
          for r in flatten([st.Resource]) : (
            !contains(flatten([st.Action]), "s3:GetObject") || r == "arn:aws:s3:::facilitator-terraform-state/production/terraform.tfstate"
            ) && (
            !contains(flatten([st.Action]), "dynamodb:GetItem") || r == "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/facilitator-terraform-locks"
            ) && (
            !contains(flatten([st.Action]), "lambda:GetFunction") || startswith(r, aws_lambda_function.balances.arn)
          )
        ] if st.Effect == "Allow"
      ]
    ]))
    error_message = "s3:GetObject reaches the state object alone, dynamodb:GetItem the lock table alone, lambda:GetFunction the balances Lambda alone."
  }

  # --- And denied outright, whatever gets attached later.

  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st
      if st.Effect == "Deny" && !can(st.Condition) && toset(flatten([st.Resource])) == toset(["*"]) &&
      length(setsubtract(toset(["secretsmanager:GetSecretValue", "ssm:GetParameter", "ssm:GetParameters", "ssm:GetParametersByPath", "ssm:GetParameterHistory"]), toset(flatten([st.Action])))) == 0
    ]) == 1
    error_message = "github_read must deny, on every resource and without condition, secretsmanager:GetSecretValue and every ssm:GetParameter* call."
  }

  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st
      if st.Effect == "Deny" && !can(st.Condition) && toset(flatten([st.Action])) == toset(["kms:Decrypt"]) && toset(flatten([st.Resource])) == toset([aws_kms_key.bazaar_admin_token.arn])
    ]) == 1
    error_message = "github_read must deny kms:Decrypt on the Bazaar token's key, unconditionally, as cicd_infra does for the user."
  }

  # --- A managed policy holds at most 6,144 characters. The mocked ARNs above leave the account out
  # of seven of them (the key, the Lambda and the topic twice, the queue once: ~100 characters), so the
  # bound keeps that margin. Measured 2026-11-18: github_read 5,238 here, github_deploy_writes 2,488.
  assert {
    condition     = length(aws_iam_policy.github_read.policy) <= 5900 && length(aws_iam_policy.github_deploy_writes.policy) <= 5900
    error_message = "A CI policy is near IAM's 6,144-character limit for a managed policy: split it before it fails to apply."
  }
}

run "the_immutable_subject_names_the_repository_by_its_ids" {
  command = plan

  variables {
    github_oidc_immutable_subject = true
  }

  plan_options {
    target = [aws_iam_role.github_plan, aws_iam_role.github_deploy]
  }

  assert {
    condition = toset(jsondecode(aws_iam_role.github_deploy.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]) == toset([
      "repo:UltravioletaDAO@172868068/x402-rs@1080715557:environment:production",
    ])
    error_message = "With the immutable subject, the deploy role trusts the production environment of owner 172868068 / repository 1080715557 alone."
  }

  assert {
    condition = toset(jsondecode(aws_iam_role.github_plan.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]) == toset([
      "repo:UltravioletaDAO@172868068/x402-rs@1080715557:pull_request",
      "repo:UltravioletaDAO@172868068/x402-rs@1080715557:ref:refs/heads/main",
    ])
    error_message = "With the immutable subject, the plan role names the repository by its ids, never by name alone."
  }
}
