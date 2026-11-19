# The CI's AWS identities by GitHub OIDC (cicd-oidc.tf), against a mocked AWS.
#
# No credentials and no state: in terraform/environments/production,
#   terraform init -backend=false && terraform test
# (CI: job "Build & test", step "Terraform NAT guards", which runs every file in tests/).
# What ci.yaml asks of these roles (which job assumes which, the environment, id-token) and that no
# other attachment or inline policy reaches them is held by tests/scripts/test_ci_oidc.py.
#
# IAM action names are case-insensitive, so every action check below compares lower case.

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
  # The balances API enters this graph (github_read names it by id); the teardown validates the
  # Lambda permission built from its execution ARN.
  mock_resource "aws_apigatewayv2_api" {
    defaults = {
      execution_arn = "arn:aws:execute-api:us-east-2::mockapi"
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
      # The policies compose their ARNs from names (cicd-oidc.tf says why), so these are no
      # dependency of theirs: applied here only to check each composed ARN against its resource.
      aws_iam_policy.cicd_infra,
      aws_iam_role.ecs_task_execution,
      aws_iam_role.ecs_task,
      aws_iam_role.balances_lambda,
      aws_lambda_function.balances,
      aws_sns_topic.alerts,
      aws_sqs_queue.alerts_backup,
      aws_s3_bucket.alb_logs,
      aws_s3_bucket.dx402_evidence,
    ]
  }

  # Distinct ARNs for the three policies the roles hold: with one mocked ARN for all, attaching the
  # wrong one passed the attachment check (measured 2026-11-18).
  override_resource {
    target = aws_iam_policy.github_read
    values = { arn = "arn:aws:iam:::policy/facilitator-github-read" }
  }
  override_resource {
    target = aws_iam_policy.github_deploy_writes
    values = { arn = "arn:aws:iam:::policy/facilitator-github-deploy-writes" }
  }
  override_resource {
    target = aws_iam_policy.cicd_infra
    values = { arn = "arn:aws:iam:::policy/facilitator-cicd-infra" }
  }

  # --- Trust: one statement each, GitHub's provider, the STS audience, an exact subject list.
  # try() everywhere a changed document could lack the attribute: in 1.9.8 && does not
  # short-circuit, and an evaluation error would hide which promise broke.

  assert {
    condition = alltrue([
      for doc in [aws_iam_role.github_plan.assume_role_policy, aws_iam_role.github_deploy.assume_role_policy] :
      try(
        length(jsondecode(doc).Statement) == 1 &&
        jsondecode(doc).Statement[0].Effect == "Allow" &&
        jsondecode(doc).Statement[0].Action == "sts:AssumeRoleWithWebIdentity" &&
        jsondecode(doc).Statement[0].Principal.Federated == "arn:aws:iam::${data.aws_caller_identity.current.account_id}:oidc-provider/token.actions.githubusercontent.com" &&
        toset(keys(jsondecode(doc).Statement[0].Condition)) == toset(["StringEquals"]) &&
        toset(keys(jsondecode(doc).Statement[0].Condition.StringEquals)) == toset(["token.actions.githubusercontent.com:aud", "token.actions.githubusercontent.com:sub"]) &&
        jsondecode(doc).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:aud"] == "sts.amazonaws.com",
        false
      )
    ])
    error_message = "Each CI role trusts GitHub's OIDC provider alone, for audience sts.amazonaws.com, with StringEquals on aud and sub only (no StringLike, no wildcard)."
  }

  assert {
    condition     = try(toset(jsondecode(aws_iam_role.github_deploy.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]), toset([])) == toset(["repo:UltravioletaDAO/x402-rs:environment:production"])
    error_message = "facilitator-github-deploy must trust the production environment of UltravioletaDAO/x402-rs and nothing else."
  }

  assert {
    condition = try(toset(jsondecode(aws_iam_role.github_plan.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]), toset([])) == toset([
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

  # --- Exactly these actions, no more: a new grant is a change to this list, read by a human.

  assert {
    condition = toset(flatten([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : [for a in flatten([st.Action]) : lower(a)] if st.Effect == "Allow"
      ])) == toset([
      "acm:describecertificate", "acm:listtagsforcertificate",
      "apigateway:get",
      "application-autoscaling:describescalabletargets", "application-autoscaling:describescalingpolicies", "application-autoscaling:listtagsforresource",
      "cloudwatch:describealarms", "cloudwatch:listtagsforresource",
      "dynamodb:describecontinuousbackups", "dynamodb:describetable", "dynamodb:describetimetolive", "dynamodb:getitem", "dynamodb:listtagsofresource",
      "ec2:describeinternetgateways", "ec2:describenetworkacls", "ec2:describenetworkinterfaces", "ec2:describeprefixlists", "ec2:describeroutetables",
      "ec2:describesecuritygroups", "ec2:describesubnets", "ec2:describevpcattribute", "ec2:describevpcendpoints", "ec2:describevpcs",
      "ecs:describeclusters", "ecs:describeservices", "ecs:describetaskdefinition",
      "elasticloadbalancing:describecapacityreservation", "elasticloadbalancing:describelistenerattributes", "elasticloadbalancing:describelisteners",
      "elasticloadbalancing:describeloadbalancerattributes", "elasticloadbalancing:describeloadbalancers", "elasticloadbalancing:describerules",
      "elasticloadbalancing:describetags", "elasticloadbalancing:describetargetgroupattributes", "elasticloadbalancing:describetargetgroups",
      "elasticloadbalancing:describetargethealth",
      "events:describerule", "events:listtagsforresource", "events:listtargetsbyrule",
      "iam:getpolicy", "iam:getpolicyversion", "iam:getrole", "iam:getrolepolicy", "iam:listattachedrolepolicies", "iam:listrolepolicies",
      "kms:describekey", "kms:getkeypolicy", "kms:getkeyrotationstatus", "kms:listaliases", "kms:listresourcetags",
      "lambda:getfunction", "lambda:getfunctioncodesigningconfig", "lambda:getpolicy", "lambda:listversionsbyfunction",
      "logs:describeloggroups", "logs:describemetricfilters", "logs:listtagsforresource",
      "route53:gethostedzone", "route53:listhostedzones", "route53:listresourcerecordsets", "route53:listtagsforresource",
      "s3:getaccelerateconfiguration", "s3:getbucketacl", "s3:getbucketcors", "s3:getbucketlogging", "s3:getbucketobjectlockconfiguration",
      "s3:getbucketownershipcontrols", "s3:getbucketpolicy", "s3:getbucketpublicaccessblock", "s3:getbucketrequestpayment", "s3:getbuckettagging",
      "s3:getbucketversioning", "s3:getbucketwebsite", "s3:getencryptionconfiguration", "s3:getlifecycleconfiguration", "s3:getobject",
      "s3:getreplicationconfiguration", "s3:listbucket",
      "secretsmanager:describesecret", "secretsmanager:getresourcepolicy",
      "sns:getsubscriptionattributes", "sns:gettopicattributes", "sns:listsubscriptionsbytopic", "sns:listtagsforresource",
      "sqs:getqueueattributes", "sqs:getqueueurl", "sqs:listqueuetags",
      "ssm:describeparameters",
    ])
    error_message = "github_read allows exactly the reads CloudTrail measured (plus the data-plane ones it does not log). A new one is added here on purpose, not by accident."
  }

  assert {
    condition = toset(flatten([
      for st in jsondecode(aws_iam_policy.github_deploy_writes.policy).Statement : [for a in flatten([st.Action]) : lower(a)] if st.Effect == "Allow"
      ])) == toset([
      "dynamodb:deleteitem", "dynamodb:putitem",
      "ecr:batchchecklayeravailability", "ecr:batchgetimage", "ecr:completelayerupload", "ecr:getauthorizationtoken",
      "ecr:getdownloadurlforlayer", "ecr:initiatelayerupload", "ecr:putimage", "ecr:uploadlayerpart",
      "ecs:deregistertaskdefinition", "ecs:registertaskdefinition", "ecs:tagresource", "ecs:updateservice",
      "elasticloadbalancing:modifylistener", "elasticloadbalancing:modifylistenerattributes",
      "elasticloadbalancing:modifyloadbalancerattributes", "elasticloadbalancing:modifytargetgroupattributes",
      "iam:passrole",
      "s3:putobject",
    ])
    error_message = "github_deploy_writes allows exactly the writes of a release (the old inline facilitator-cicd, narrowed). A new one is added here on purpose."
  }

  # --- And each statement exactly: its effect, its actions and its resources. The sets above catch a
  # new action; these catch an action moved to a "*" statement and a resource widened (repository/*,
  # service/*/*, bucket/*), which keep the sets the same.

  assert {
    condition = {
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st.Sid => {
        effect    = st.Effect
        actions   = toset([for a in flatten([st.Action]) : lower(a)])
        resources = toset(flatten([st.Resource]))
      }
      } == {
      TerraformStateRead = { effect = "Allow", actions = toset(["s3:getobject"]), resources = toset(["arn:aws:s3:::facilitator-terraform-state/production/terraform.tfstate"]) }
      BucketList = { effect = "Allow", actions = toset(["s3:listbucket"]), resources = toset([
        "arn:aws:s3:::facilitator-terraform-state",
        "arn:aws:s3:::${aws_s3_bucket.alb_logs.bucket}",
        "arn:aws:s3:::${aws_s3_bucket.dx402_evidence.bucket}",
      ]) }
      TerraformLockRead = { effect = "Allow", actions = toset(["dynamodb:getitem", "dynamodb:describetable"]), resources = toset([
        "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/facilitator-terraform-locks",
      ]) }
      BucketConfigRead = { effect = "Allow", actions = toset([
        "s3:getaccelerateconfiguration", "s3:getbucketacl", "s3:getbucketcors", "s3:getbucketlogging", "s3:getbucketobjectlockconfiguration",
        "s3:getbucketownershipcontrols", "s3:getbucketpolicy", "s3:getbucketpublicaccessblock", "s3:getbucketrequestpayment",
        "s3:getbuckettagging", "s3:getbucketversioning", "s3:getbucketwebsite", "s3:getencryptionconfiguration",
        "s3:getlifecycleconfiguration", "s3:getreplicationconfiguration",
      ]), resources = toset(["arn:aws:s3:::facilitator-*"]) }
      MetadataRead = { effect = "Allow", actions = toset([
        "acm:describecertificate", "acm:listtagsforcertificate",
        "application-autoscaling:describescalabletargets", "application-autoscaling:describescalingpolicies", "application-autoscaling:listtagsforresource",
        "cloudwatch:describealarms", "cloudwatch:listtagsforresource",
        "ec2:describeinternetgateways", "ec2:describenetworkacls", "ec2:describenetworkinterfaces", "ec2:describeprefixlists", "ec2:describeroutetables",
        "ec2:describesecuritygroups", "ec2:describesubnets", "ec2:describevpcattribute", "ec2:describevpcendpoints", "ec2:describevpcs",
        "elasticloadbalancing:describecapacityreservation", "elasticloadbalancing:describelistenerattributes", "elasticloadbalancing:describelisteners",
        "elasticloadbalancing:describeloadbalancerattributes", "elasticloadbalancing:describeloadbalancers", "elasticloadbalancing:describerules",
        "elasticloadbalancing:describetags", "elasticloadbalancing:describetargetgroupattributes", "elasticloadbalancing:describetargetgroups",
        "elasticloadbalancing:describetargethealth",
        "kms:listaliases", "logs:describeloggroups", "route53:listhostedzones", "ssm:describeparameters",
      ]), resources = toset(["*"]) }
      TaskDefinitionRead = { effect = "Allow", actions = toset(["ecs:describetaskdefinition"]), resources = toset(["*"]) }
      HostedZoneRead = { effect = "Allow", actions = toset(["route53:gethostedzone", "route53:listresourcerecordsets", "route53:listtagsforresource"]), resources = toset([
        "arn:aws:route53:::hostedzone/${data.aws_route53_zone.main.zone_id}",
      ]) }
      EcsRead = { effect = "Allow", actions = toset(["ecs:describeclusters", "ecs:describeservices"]), resources = toset([
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:cluster/facilitator-production",
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:service/facilitator-production/*",
      ]) }
      DynamoTableMetadataRead = { effect = "Allow", actions = toset([
        "dynamodb:describecontinuousbackups", "dynamodb:describetable", "dynamodb:describetimetolive", "dynamodb:listtagsofresource",
      ]), resources = toset(["arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/*"]) }
      LogGroupMetadataRead = { effect = "Allow", actions = toset(["logs:describemetricfilters", "logs:listtagsforresource"]), resources = toset([
        "arn:aws:logs:us-east-2:${data.aws_caller_identity.current.account_id}:log-group:*",
      ]) }
      IamRead = { effect = "Allow", actions = toset([
        "iam:getpolicy", "iam:getpolicyversion", "iam:getrole", "iam:getrolepolicy", "iam:listattachedrolepolicies", "iam:listrolepolicies",
        ]), resources = toset([
        "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/facilitator-*",
        "arn:aws:iam::${data.aws_caller_identity.current.account_id}:policy/facilitator-*",
      ]) }
      KmsKeyRead = { effect = "Allow", actions = toset(["kms:describekey", "kms:getkeypolicy", "kms:getkeyrotationstatus", "kms:listresourcetags"]), resources = toset([aws_kms_key.bazaar_admin_token.arn]) }
      BalancesLambdaRead = { effect = "Allow", actions = toset([
        "lambda:getfunction", "lambda:getfunctioncodesigningconfig", "lambda:getpolicy", "lambda:listversionsbyfunction",
        ]), resources = toset([
        "arn:aws:lambda:us-east-2:${data.aws_caller_identity.current.account_id}:function:${aws_lambda_function.balances.function_name}",
        "arn:aws:lambda:us-east-2:${data.aws_caller_identity.current.account_id}:function:${aws_lambda_function.balances.function_name}:*",
      ]) }
      BalancesApiRead = { effect = "Allow", actions = toset(["apigateway:get"]), resources = toset([
        "arn:aws:apigateway:us-east-2::/apis/${aws_apigatewayv2_api.balances.id}",
        "arn:aws:apigateway:us-east-2::/apis/${aws_apigatewayv2_api.balances.id}/*",
      ]) }
      ScheduleRead = { effect = "Allow", actions = toset(["events:describerule", "events:listtagsforresource", "events:listtargetsbyrule"]), resources = toset([
        "arn:aws:events:us-east-2:${data.aws_caller_identity.current.account_id}:rule/facilitator-production-*",
      ]) }
      AlertTopicRead = { effect = "Allow", actions = toset([
        "sns:getsubscriptionattributes", "sns:gettopicattributes", "sns:listsubscriptionsbytopic", "sns:listtagsforresource",
        ]), resources = toset([
        "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:${aws_sns_topic.alerts.name}",
        "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:${aws_sns_topic.alerts.name}:*",
      ]) }
      AlertsBackupQueueRead = { effect = "Allow", actions = toset(["sqs:getqueueattributes", "sqs:getqueueurl", "sqs:listqueuetags"]), resources = toset([
        "arn:aws:sqs:us-east-2:${data.aws_caller_identity.current.account_id}:${aws_sqs_queue.alerts_backup.name}",
      ]) }
      SecretMetadataRead = { effect = "Allow", actions = toset(["secretsmanager:describesecret", "secretsmanager:getresourcepolicy"]), resources = toset([
        "arn:aws:secretsmanager:us-east-2:${data.aws_caller_identity.current.account_id}:secret:facilitator-*",
      ]) }
      DenySecretValueReads = { effect = "Deny", actions = toset([
        "secretsmanager:getsecretvalue", "ssm:getparameter", "ssm:getparameterhistory", "ssm:getparameters", "ssm:getparametersbypath",
      ]), resources = toset(["*"]) }
      DenyBazaarTokenDecrypt = { effect = "Deny", actions = toset(["kms:decrypt"]), resources = toset([aws_kms_key.bazaar_admin_token.arn]) }
    }
    error_message = "Each github_read statement grants exactly its actions on exactly its resources: an action moved to a \"*\" statement, or a resource widened, is a change made here on purpose."
  }

  assert {
    condition = {
      for st in jsondecode(aws_iam_policy.github_deploy_writes.policy).Statement : st.Sid => {
        effect    = st.Effect
        actions   = toset([for a in flatten([st.Action]) : lower(a)])
        resources = toset(flatten([st.Resource]))
      }
      } == {
      EcrLogin = { effect = "Allow", actions = toset(["ecr:getauthorizationtoken"]), resources = toset(["*"]) }
      EcrPushFacilitator = { effect = "Allow", actions = toset([
        "ecr:batchchecklayeravailability", "ecr:batchgetimage", "ecr:completelayerupload", "ecr:getdownloadurlforlayer",
        "ecr:initiatelayerupload", "ecr:putimage", "ecr:uploadlayerpart",
      ]), resources = toset(["arn:aws:ecr:us-east-2:${data.aws_caller_identity.current.account_id}:repository/facilitator"]) }
      EcsRegisterTaskDefinition = { effect = "Allow", actions = toset(["ecs:registertaskdefinition"]), resources = toset([
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:task-definition/facilitator-production:*",
      ]) }
      EcsDeregisterTaskDefinition = { effect = "Allow", actions = toset(["ecs:deregistertaskdefinition"]), resources = toset(["*"]) }
      EcsRollService = { effect = "Allow", actions = toset(["ecs:updateservice"]), resources = toset([
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:service/facilitator-production/facilitator-production",
      ]) }
      EcsTag = { effect = "Allow", actions = toset(["ecs:tagresource"]), resources = toset([
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:cluster/facilitator-production",
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:service/facilitator-production/facilitator-production",
        "arn:aws:ecs:us-east-2:${data.aws_caller_identity.current.account_id}:task-definition/facilitator-production:*",
      ]) }
      PassRoleToEcsTasks = { effect = "Allow", actions = toset(["iam:passrole"]), resources = toset([
        "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.ecs_task_execution.name}",
        "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.ecs_task.name}",
      ]) }
      PassRoleToBalancesLambda = { effect = "Allow", actions = toset(["iam:passrole"]), resources = toset([
        "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.balances_lambda.name}",
      ]) }
      AlbModify = { effect = "Allow", actions = toset([
        "elasticloadbalancing:modifylistener", "elasticloadbalancing:modifylistenerattributes",
        "elasticloadbalancing:modifyloadbalancerattributes", "elasticloadbalancing:modifytargetgroupattributes",
        ]), resources = toset([
        "arn:aws:elasticloadbalancing:us-east-2:${data.aws_caller_identity.current.account_id}:loadbalancer/app/facilitator-production/*",
        "arn:aws:elasticloadbalancing:us-east-2:${data.aws_caller_identity.current.account_id}:listener/app/facilitator-production/*",
        "arn:aws:elasticloadbalancing:us-east-2:${data.aws_caller_identity.current.account_id}:targetgroup/facilitator-production*/*",
      ]) }
      TerraformStateWrite = { effect = "Allow", actions = toset(["s3:putobject"]), resources = toset([
        "arn:aws:s3:::facilitator-terraform-state/production/terraform.tfstate",
      ]) }
      TerraformLockWrite = { effect = "Allow", actions = toset(["dynamodb:putitem", "dynamodb:deleteitem"]), resources = toset([
        "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/facilitator-terraform-locks",
      ]) }
    }
    error_message = "Each github_deploy_writes statement grants exactly its actions on exactly its resources: a write widened to another project's repository, service, task family, bucket or table is a change made here on purpose."
  }

  # --- PassRole, in either policy: the three roles of a release, each only to the service that runs it.

  assert {
    condition = alltrue(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement :
        # try(): in 1.9.8 || does not short-circuit, so a statement without Condition must not error here.
        !contains([for a in flatten([st.Action]) : lower(a)], "iam:passrole") || (
          st.Effect == "Allow" &&
          length(flatten([st.Action])) == 1 &&
          try(toset(keys(st.Condition)), toset([])) == toset(["StringEquals"]) &&
          try(toset(keys(st.Condition.StringEquals)), toset([])) == toset(["iam:PassedToService"])
        )
      ]
    ]))
    error_message = "Every iam:PassRole grant stands alone in its statement, conditioned on iam:PassedToService."
  }

  assert {
    condition = toset(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement : [
          for r in flatten([st.Resource]) : "${r} -> ${try(st.Condition.StringEquals["iam:PassedToService"], "any service")}"
        ] if contains([for a in flatten([st.Action]) : lower(a)], "iam:passrole")
      ]
      ])) == toset([
      "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.ecs_task_execution.name} -> ecs-tasks.amazonaws.com",
      "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.ecs_task.name} -> ecs-tasks.amazonaws.com",
      "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${aws_iam_role.balances_lambda.name} -> lambda.amazonaws.com",
    ])
    error_message = "PassRole reaches exactly the execution and task roles (to ECS tasks) and the balances Lambda role (to Lambda), by the names main.tf and lambda-balances.tf give them."
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
          can(regex("^[a-z0-9-]+:(describe|list)", lower(a))) || contains(["ecr:getauthorizationtoken", "ecs:deregistertaskdefinition"], lower(a))
        ] if st.Effect == "Allow" && contains(flatten([st.Resource]), "*")
      ]
    ]))
    error_message = "Resource \"*\" is allowed only for Describe/List calls, ECR login and DeregisterTaskDefinition, which IAM cannot scope."
  }

  # --- Reads that return data reach this stack's own resources, by the names the resources declare.

  assert {
    condition = alltrue(flatten([
      for doc in [aws_iam_policy.github_read.policy, aws_iam_policy.github_deploy_writes.policy] : [
        for st in jsondecode(doc).Statement : [
          for r in flatten([st.Resource]) : (
            !contains([for a in flatten([st.Action]) : lower(a)], "s3:getobject") ||
            r == "arn:aws:s3:::facilitator-terraform-state/production/terraform.tfstate"
            ) && (
            !contains([for a in flatten([st.Action]) : lower(a)], "dynamodb:getitem") ||
            r == "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/facilitator-terraform-locks"
            ) && (
            !contains([for a in flatten([st.Action]) : lower(a)], "lambda:getfunction") ||
            contains([
              "arn:aws:lambda:us-east-2:${data.aws_caller_identity.current.account_id}:function:${aws_lambda_function.balances.function_name}",
              "arn:aws:lambda:us-east-2:${data.aws_caller_identity.current.account_id}:function:${aws_lambda_function.balances.function_name}:*",
            ], r)
            ) && (
            !contains([for a in flatten([st.Action]) : lower(a)], "apigateway:get") ||
            startswith(r, "arn:aws:apigateway:us-east-2::/apis/${aws_apigatewayv2_api.balances.id}")
            ) && (
            !contains([for a in flatten([st.Action]) : lower(a)], "s3:listbucket") ||
            contains([
              "arn:aws:s3:::facilitator-terraform-state",
              "arn:aws:s3:::${aws_s3_bucket.alb_logs.bucket}",
              "arn:aws:s3:::${aws_s3_bucket.dx402_evidence.bucket}",
            ], r)
          )
        ] if st.Effect == "Allow"
      ]
    ]))
    error_message = "s3:GetObject reaches the state object alone, dynamodb:GetItem the lock table, lambda:GetFunction the balances Lambda, apigateway:GET the balances API, s3:ListBucket the state and the two buckets declared here."
  }

  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st
      if st.Effect == "Allow" && contains([for a in flatten([st.Action]) : lower(a)], "sqs:getqueueattributes") &&
      toset(flatten([st.Resource])) == toset(["arn:aws:sqs:us-east-2:${data.aws_caller_identity.current.account_id}:${aws_sqs_queue.alerts_backup.name}"])
      ]) == 1 && length([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st
      if st.Effect == "Allow" && contains([for a in flatten([st.Action]) : lower(a)], "sns:gettopicattributes") &&
      toset(flatten([st.Resource])) == toset([
        "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:${aws_sns_topic.alerts.name}",
        "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:${aws_sns_topic.alerts.name}:*",
      ])
    ]) == 1
    error_message = "The queue and topic reads name the alerts queue and topic, by the names alerts.tf gives them."
  }

  # --- And denied outright, whatever gets attached later.

  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st
      if st.Effect == "Deny" && !can(st.Condition) && toset(flatten([st.Resource])) == toset(["*"]) &&
      length(setsubtract(toset(["secretsmanager:getsecretvalue", "ssm:getparameter", "ssm:getparameters", "ssm:getparametersbypath", "ssm:getparameterhistory"]), toset([for a in flatten([st.Action]) : lower(a)]))) == 0
    ]) == 1
    error_message = "github_read must deny, on every resource and without condition, secretsmanager:GetSecretValue and every ssm:GetParameter* call."
  }

  assert {
    condition = length([
      for st in jsondecode(aws_iam_policy.github_read.policy).Statement : st
      if st.Effect == "Deny" && !can(st.Condition) && toset([for a in flatten([st.Action]) : lower(a)]) == toset(["kms:decrypt"]) && toset(flatten([st.Resource])) == toset([aws_kms_key.bazaar_admin_token.arn])
    ]) == 1
    error_message = "github_read must deny kms:Decrypt on the Bazaar token's key, unconditionally, as cicd_infra does for the user."
  }

  # --- A managed policy holds at most 6,144 characters. The mocked key ARN leaves the account out
  # (~12 characters a mention), so the bound keeps a margin. Measured 2026-11-18 before the API and
  # zone were narrowed: github_read 5,238 here, github_deploy_writes 2,488.
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
    condition = try(toset(jsondecode(aws_iam_role.github_deploy.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]), toset([])) == toset([
      "repo:UltravioletaDAO@172868068/x402-rs@1080715557:environment:production",
    ])
    error_message = "With the immutable subject, the deploy role trusts the production environment of owner 172868068 / repository 1080715557 alone."
  }

  assert {
    condition = try(toset(jsondecode(aws_iam_role.github_plan.assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]), toset([])) == toset([
      "repo:UltravioletaDAO@172868068/x402-rs@1080715557:pull_request",
      "repo:UltravioletaDAO@172868068/x402-rs@1080715557:ref:refs/heads/main",
    ])
    error_message = "With the immutable subject, the plan role names the repository by its ids, never by name alone."
  }
}
