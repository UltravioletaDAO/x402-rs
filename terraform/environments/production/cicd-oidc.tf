# ============================================================================
# The CI's AWS identities, by GitHub OIDC -- DECLARED here, APPLIED by hand
# ============================================================================
#
# Until this file, the pipeline authenticated with a long-lived access key of
# the IAM user github-actions-facilitator-deploy, stored as two repository
# secrets of a PUBLIC repository. That user holds the AWS-managed ReadOnlyAccess
# (which reads every SSM SecureString on the default key, every object of every
# bucket, every log, every Lambda environment of the account) plus an inline
# policy with ecr/ecs writes and iam:PassRole on "*" (docs/CICD_SETUP.md).
#
# Two roles replace it, each assumed with the job's OIDC token, nothing stored:
#
#   facilitator-github-plan    the drift gate (ci.yaml job `plan`). Read-only.
#                              Trusted for pull_request and for main's ref:
#                              the gate runs before a merge on purpose
#                              (tests/scripts/test_ci_deploy_job.py), so it
#                              cannot sit behind the production environment.
#   facilitator-github-deploy  the release (ci.yaml job `deploy`). Trusted
#                              ONLY for the `production` environment. What
#                              keeps a job out of it is GitHub-side, not here:
#                              the environment's branch rule (main only) AND
#                              main itself being protected -- with main open,
#                              anyone with write access pushes a workflow to
#                              main and gets the token (measured 2026-11-18:
#                              main unprotected, no ruleset). A required
#                              reviewer on the environment closes that too.
#
# Neither role holds ReadOnlyAccess. What they read was MEASURED, not guessed:
# CloudTrail event history of the CI user over the release of 2026-11-15
# 17:00-18:05Z (run 43740441328 plus its push run 43740406919: two full plans,
# the image deploy, the Lambda, observability and bucket steps) -- 1734 events,
# 97 distinct calls, no AccessDenied. That covers management events only, so
# the data-plane reads every plan makes and CloudTrail never logs are added by
# name: the state object and its bucket listing, the lock table's digest item
# (GetItem, read even with -lock=false), HeadBucket (s3:ListBucket) on the two
# buckets declared here, and the SQS queue's attributes. Data reads of anything
# else -- ssm:GetParameter*, secretsmanager:GetSecretValue, s3:GetObject
# outside the state, DynamoDB items, log events -- are not granted, and the
# first two are denied outright whatever is attached later.
#
# Resources counted off today (enable_observability, enable_nat_gateway, the
# ECR lifecycle policy) made no call in that window and get no read here:
# switching one on makes the drift gate fail loudly on AccessDenied, with the
# call named, and the grant goes in the same change.
#
# One read IAM cannot narrow: ecs:DescribeTaskDefinition authorizes on "*"
# only, and it returns the plaintext `environment` of every task definition in
# the account. Anyone who can open a pull request from a branch of this repo can
# assume the plan role, so a secret written in plain text into ANOTHER project's
# task definition is readable from here (it was with ReadOnlyAccess too).
# Secrets belong in `secrets`/valueFrom, never in `environment`.
#
# ARNs are composed from names, not read from the resources, on purpose: a
# reference makes a hand `-target` apply of these roles apply every pending
# change of what it references too -- the balances Lambda's zip hashes
# differently from a CRLF or DrvFs checkout, so it would be redeployed from the
# operator's copy. tests/github_oidc.tftest.hcl ties each composed ARN to the
# resource's own name. The KMS key and the API and zone ids are not knowable as
# text and are referenced; neither has pending changes outside a deploy.
#
# ---------------------------------------------------------------------------
# WHY THIS IS NOT IN CI'S -target LIST, AND MUST NEVER BE
# ---------------------------------------------------------------------------
#
# Same rule as cicd-iam-policy.tf: if CI could apply its own trust and grants,
# DenyPrivilegeEscalation would be decorative. CI cannot: iam:CreateRole,
# iam:CreatePolicy(Version) and iam:UpdateAssumeRolePolicy are denied to it
# outside the task role. Apply by hand, with a human's credentials, on 1.9.8:
#
#   terraform apply \
#     -target=aws_iam_role.github_plan -target=aws_iam_role.github_deploy \
#     -target=aws_iam_policy.github_read -target=aws_iam_policy.github_deploy_writes \
#     -target=aws_iam_role_policy_attachment.github_plan_read \
#     -target=aws_iam_role_policy_attachment.github_deploy_read \
#     -target=aws_iam_role_policy_attachment.github_deploy_writes \
#     -target=aws_iam_role_policy_attachment.github_deploy_infra
#
# The order, all of it BEFORE the change that switches ci.yaml to the roles is
# merged (the old ci.yaml keeps using the key meanwhile; the pull request's own
# run then proves the plan role):
#   1. on GitHub, the environment `production` with deployment branches = main
#      only, and a required reviewer; and main protected (no direct push, no
#      force-push). Creating the environment first matters: a job that names a
#      missing environment creates it with no rule at all;
#   2. this apply;
#   3. the repository secret AWS_ACCOUNT_ID (a secret, not a variable: GitHub
#      masks it in the public logs);
#   4. merge; after one green push run and one green release on the roles,
#      deactivate and delete the user's key and the secrets
#      AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY. Until then the key is still
#      readable by any workflow of any branch: do not wait for a release.
# A manual "Run workflow" with no AWS_ACCOUNT_ID fails in preflight instead of
# going green with nothing deployed.
#
# The OIDC provider for token.actions.githubusercontent.com already exists in
# the account and is owned by another stack, so it is referenced by its ARN,
# never declared or read here.

variable "github_oidc_immutable_subject" {
  description = "Trust the immutable OIDC subject (repo:OWNER@OWNER-ID/REPO@REPO-ID:...) instead of the name-based one. Flip together with use_immutable_subject on the repository (PUT /repos/{owner}/{repo}/actions/oidc/customization/sub)."
  type        = bool
  default     = false
}

locals {
  # The repository's name, and its immutable ids (GET /repos/UltravioletaDAO/x402-rs: id and owner.id,
  # 2026-11-18). The ids survive a rename; the name can be re-registered by whoever owns it next.
  github_repository     = "UltravioletaDAO/x402-rs"
  github_repository_ids = "UltravioletaDAO@172868068/x402-rs@1080715557"
  github_sub_repo       = var.github_oidc_immutable_subject ? "repo:${local.github_repository_ids}" : "repo:${local.github_repository}"

  # A job that names an environment gets environment:<name> as its context; one that does not gets
  # pull_request on a pull request and ref:<ref> otherwise (GitHub's OIDC reference).
  github_deploy_subjects = ["${local.github_sub_repo}:environment:production"]
  github_plan_subjects = [
    "${local.github_sub_repo}:pull_request",
    "${local.github_sub_repo}:ref:refs/heads/main",
  ]

  github_oidc_provider_arn = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:oidc-provider/token.actions.githubusercontent.com"

  ci_account = data.aws_caller_identity.current.account_id
  ci_state = {
    bucket = "facilitator-terraform-state"
    key    = "production/terraform.tfstate"
    table  = "facilitator-terraform-locks"
  }

  # Composed from the names the resources declare (see the header for why not referenced);
  # tests/github_oidc.tftest.hcl fails if a resource is renamed and these are not.
  ci_prefix = "facilitator-${var.environment}"
  ci_arn = {
    execution_role       = "arn:aws:iam::${local.ci_account}:role/${local.ci_prefix}-ecs-execution"
    task_role            = "arn:aws:iam::${local.ci_account}:role/${local.ci_prefix}-ecs-task"
    balances_lambda_role = "arn:aws:iam::${local.ci_account}:role/${local.ci_prefix}-balances-lambda"
    balances_lambda      = "arn:aws:lambda:us-east-2:${local.ci_account}:function:${local.ci_prefix}-balances"
    alerts_topic         = "arn:aws:sns:us-east-2:${local.ci_account}:${local.ci_prefix}-alerts"
    alerts_queue         = "arn:aws:sqs:us-east-2:${local.ci_account}:${local.ci_prefix}-alerts-backup"
    alb_logs_bucket      = "arn:aws:s3:::${local.alb_access_logs_bucket_name}"
    dx402_bucket         = "arn:aws:s3:::${local.dx402_bucket_name}"
  }
}

resource "aws_iam_role" "github_plan" {
  name                 = "facilitator-github-plan"
  description          = "GitHub Actions, x402-rs drift gate (ci.yaml job plan): read-only, by OIDC."
  max_session_duration = 3600

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "GitHubActionsPlan"
      Effect    = "Allow"
      Principal = { Federated = local.github_oidc_provider_arn }
      Action    = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = {
          "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
          "token.actions.githubusercontent.com:sub" = local.github_plan_subjects
        }
      }
    }]
  })
}

resource "aws_iam_role" "github_deploy" {
  name                 = "facilitator-github-deploy"
  description          = "GitHub Actions, x402-rs release (ci.yaml job deploy): environment production only, by OIDC."
  max_session_duration = 3600

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "GitHubActionsProductionEnvironment"
      Effect    = "Allow"
      Principal = { Federated = local.github_oidc_provider_arn }
      Action    = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = {
          "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
          "token.actions.githubusercontent.com:sub" = local.github_deploy_subjects
        }
      }
    }]
  })
}

# What a plan reads. Both roles carry it: the release's applies refresh the same resources.
resource "aws_iam_policy" "github_read" {
  name        = "facilitator-github-read"
  description = "What terraform plan of the facilitator stack reads, measured in CloudTrail; no secret, parameter or data reads."

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "TerraformStateRead"
        Effect = "Allow"
        Action = ["s3:GetObject"]
        Resource = "arn:aws:s3:::${local.ci_state.bucket}/${local.ci_state.key}"
      },
      {
        # The backend lists the bucket; HeadBucket is s3:ListBucket on the two buckets declared here.
        Sid    = "BucketList"
        Effect = "Allow"
        Action = ["s3:ListBucket"]
        Resource = [
          "arn:aws:s3:::${local.ci_state.bucket}",
          local.ci_arn.alb_logs_bucket,
          local.ci_arn.dx402_bucket,
        ]
      },
      {
        # The digest item the S3 backend checks on every read of the state.
        Sid      = "TerraformLockRead"
        Effect   = "Allow"
        Action   = ["dynamodb:GetItem", "dynamodb:DescribeTable"]
        Resource = "arn:aws:dynamodb:us-east-2:${local.ci_account}:table/${local.ci_state.table}"
      },
      {
        # Bucket configuration, never an object. Every bucket this stack configures is facilitator-*.
        Sid    = "BucketConfigRead"
        Effect = "Allow"
        Action = [
          "s3:GetAccelerateConfiguration",
          "s3:GetBucketAcl",
          "s3:GetBucketCORS",
          "s3:GetBucketLogging",
          "s3:GetBucketObjectLockConfiguration",
          "s3:GetBucketOwnershipControls",
          "s3:GetBucketPolicy",
          "s3:GetBucketPublicAccessBlock",
          "s3:GetBucketRequestPayment",
          "s3:GetBucketTagging",
          "s3:GetBucketVersioning",
          "s3:GetBucketWebsite",
          "s3:GetEncryptionConfiguration",
          "s3:GetLifecycleConfiguration",
          "s3:GetReplicationConfiguration",
        ]
        Resource = "arn:aws:s3:::facilitator-*"
      },
      {
        # Describe calls that IAM authorizes on "*" only, or that return metadata alone.
        Sid    = "MetadataRead"
        Effect = "Allow"
        Action = [
          "acm:DescribeCertificate",
          "acm:ListTagsForCertificate",
          "application-autoscaling:DescribeScalableTargets",
          "application-autoscaling:DescribeScalingPolicies",
          "application-autoscaling:ListTagsForResource",
          "cloudwatch:DescribeAlarms",
          "cloudwatch:ListTagsForResource",
          "ec2:DescribeInternetGateways",
          "ec2:DescribeNetworkAcls",
          "ec2:DescribeNetworkInterfaces",
          "ec2:DescribePrefixLists",
          "ec2:DescribeRouteTables",
          "ec2:DescribeSecurityGroups",
          "ec2:DescribeSubnets",
          "ec2:DescribeVpcAttribute",
          "ec2:DescribeVpcEndpoints",
          "ec2:DescribeVpcs",
          "elasticloadbalancing:DescribeCapacityReservation",
          "elasticloadbalancing:DescribeListenerAttributes",
          "elasticloadbalancing:DescribeListeners",
          "elasticloadbalancing:DescribeLoadBalancerAttributes",
          "elasticloadbalancing:DescribeLoadBalancers",
          "elasticloadbalancing:DescribeRules",
          "elasticloadbalancing:DescribeTags",
          "elasticloadbalancing:DescribeTargetGroupAttributes",
          "elasticloadbalancing:DescribeTargetGroups",
          "elasticloadbalancing:DescribeTargetHealth",
          "kms:ListAliases",
          "logs:DescribeLogGroups",
          "route53:ListHostedZones",
          "ssm:DescribeParameters",
        ]
        Resource = "*"
      },
      {
        # Not metadata: it returns the task definition's plaintext `environment`, and IAM authorizes
        # it on "*" only. The residual the header describes.
        Sid      = "TaskDefinitionRead"
        Effect   = "Allow"
        Action   = ["ecs:DescribeTaskDefinition"]
        Resource = "*"
      },
      {
        # The one zone this stack reads and writes records in.
        Sid      = "HostedZoneRead"
        Effect   = "Allow"
        Action   = ["route53:GetHostedZone", "route53:ListResourceRecordSets", "route53:ListTagsForResource"]
        Resource = "arn:aws:route53:::hostedzone/${data.aws_route53_zone.main.zone_id}"
      },
      {
        Sid    = "EcsRead"
        Effect = "Allow"
        Action = ["ecs:DescribeClusters", "ecs:DescribeServices"]
        Resource = [
          "arn:aws:ecs:us-east-2:${local.ci_account}:cluster/facilitator-production",
          "arn:aws:ecs:us-east-2:${local.ci_account}:service/facilitator-production/*",
        ]
      },
      {
        Sid    = "DynamoTableMetadataRead"
        Effect = "Allow"
        Action = [
          "dynamodb:DescribeContinuousBackups",
          "dynamodb:DescribeTable",
          "dynamodb:DescribeTimeToLive",
          "dynamodb:ListTagsOfResource",
        ]
        Resource = "arn:aws:dynamodb:us-east-2:${local.ci_account}:table/*"
      },
      {
        Sid      = "LogGroupMetadataRead"
        Effect   = "Allow"
        Action   = ["logs:DescribeMetricFilters", "logs:ListTagsForResource"]
        Resource = "arn:aws:logs:us-east-2:${local.ci_account}:log-group:*"
      },
      {
        Sid    = "IamRead"
        Effect = "Allow"
        Action = [
          "iam:GetPolicy",
          "iam:GetPolicyVersion",
          "iam:GetRole",
          "iam:GetRolePolicy",
          "iam:ListAttachedRolePolicies",
          "iam:ListRolePolicies",
        ]
        Resource = [
          "arn:aws:iam::${local.ci_account}:role/facilitator-*",
          "arn:aws:iam::${local.ci_account}:policy/facilitator-*",
        ]
      },
      {
        Sid      = "KmsKeyRead"
        Effect   = "Allow"
        Action   = ["kms:DescribeKey", "kms:GetKeyPolicy", "kms:GetKeyRotationStatus", "kms:ListResourceTags"]
        Resource = aws_kms_key.bazaar_admin_token.arn
      },
      {
        # GetFunction returns the function's environment, decrypted by Lambda: this function only.
        Sid    = "BalancesLambdaRead"
        Effect = "Allow"
        Action = [
          "lambda:GetFunction",
          "lambda:GetFunctionCodeSigningConfig",
          "lambda:GetPolicy",
          "lambda:ListVersionsByFunction",
        ]
        Resource = [
          local.ci_arn.balances_lambda,
          "${local.ci_arn.balances_lambda}:*",
        ]
      },
      {
        # The balances HTTP API alone: GET on /apis/<its id>. Never /apikeys (GET returns key
        # values), never another project's API (its integrations and stage variables).
        Sid    = "BalancesApiRead"
        Effect = "Allow"
        Action = ["apigateway:GET"]
        Resource = [
          "arn:aws:apigateway:us-east-2::/apis/${aws_apigatewayv2_api.balances.id}",
          "arn:aws:apigateway:us-east-2::/apis/${aws_apigatewayv2_api.balances.id}/*",
        ]
      },
      {
        Sid      = "ScheduleRead"
        Effect   = "Allow"
        Action   = ["events:DescribeRule", "events:ListTagsForResource", "events:ListTargetsByRule"]
        Resource = "arn:aws:events:us-east-2:${local.ci_account}:rule/facilitator-production-*"
      },
      {
        Sid    = "AlertTopicRead"
        Effect = "Allow"
        Action = [
          "sns:GetSubscriptionAttributes",
          "sns:GetTopicAttributes",
          "sns:ListSubscriptionsByTopic",
          "sns:ListTagsForResource",
        ]
        Resource = [local.ci_arn.alerts_topic, "${local.ci_arn.alerts_topic}:*"]
      },
      {
        # Not in CloudTrail's event history, read by every refresh of the queue all the same.
        Sid      = "AlertsBackupQueueRead"
        Effect   = "Allow"
        Action   = ["sqs:GetQueueAttributes", "sqs:GetQueueUrl", "sqs:ListQueueTags"]
        Resource = local.ci_arn.alerts_queue
      },
      {
        # DescribeSecret is metadata; the value is GetSecretValue, denied below.
        Sid      = "SecretMetadataRead"
        Effect   = "Allow"
        Action   = ["secretsmanager:DescribeSecret", "secretsmanager:GetResourcePolicy"]
        Resource = "arn:aws:secretsmanager:us-east-2:${local.ci_account}:secret:facilitator-*"
      },
      {
        # Whatever is attached to these roles later: no secret value, no parameter value, and not the
        # Bazaar token's key (cicd-iam-policy.tf denies the same to the user).
        Sid    = "DenySecretValueReads"
        Effect = "Deny"
        Action = [
          "secretsmanager:GetSecretValue",
          "ssm:GetParameter",
          "ssm:GetParameterHistory",
          "ssm:GetParameters",
          "ssm:GetParametersByPath",
        ]
        Resource = "*"
      },
      {
        Sid      = "DenyBazaarTokenDecrypt"
        Effect   = "Deny"
        Action   = ["kms:Decrypt"]
        Resource = aws_kms_key.bazaar_admin_token.arn
      },
    ]
  })
}

# What a release writes beyond facilitator-cicd-infra: the inline facilitator-cicd of the CI user
# (docs/CICD_SETUP.md), each grant narrowed from "*" to the facilitator's own resources.
resource "aws_iam_policy" "github_deploy_writes" {
  name        = "facilitator-github-deploy-writes"
  description = "Image push, task definition, service roll and state write of the facilitator release; PassRole to its three roles only."

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "EcrLogin"
        Effect   = "Allow"
        Action   = ["ecr:GetAuthorizationToken"]
        Resource = "*"
      },
      {
        Sid    = "EcrPushFacilitator"
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:BatchGetImage",
          "ecr:CompleteLayerUpload",
          "ecr:GetDownloadUrlForLayer",
          "ecr:InitiateLayerUpload",
          "ecr:PutImage",
          "ecr:UploadLayerPart",
        ]
        Resource = "arn:aws:ecr:us-east-2:${local.ci_account}:repository/${var.ecr_repository_name}"
      },
      {
        Sid      = "EcsRegisterTaskDefinition"
        Effect   = "Allow"
        Action   = ["ecs:RegisterTaskDefinition"]
        Resource = "arn:aws:ecs:us-east-2:${local.ci_account}:task-definition/facilitator-production:*"
      },
      {
        # IAM authorizes DeregisterTaskDefinition on "*" only.
        Sid      = "EcsDeregisterTaskDefinition"
        Effect   = "Allow"
        Action   = ["ecs:DeregisterTaskDefinition"]
        Resource = "*"
      },
      {
        Sid      = "EcsRollService"
        Effect   = "Allow"
        Action   = ["ecs:UpdateService"]
        Resource = "arn:aws:ecs:us-east-2:${local.ci_account}:service/facilitator-production/facilitator-production"
      },
      {
        Sid    = "EcsTag"
        Effect = "Allow"
        Action = ["ecs:TagResource"]
        Resource = [
          "arn:aws:ecs:us-east-2:${local.ci_account}:cluster/facilitator-production",
          "arn:aws:ecs:us-east-2:${local.ci_account}:service/facilitator-production/facilitator-production",
          "arn:aws:ecs:us-east-2:${local.ci_account}:task-definition/facilitator-production:*",
        ]
      },
      {
        # The task definition names both roles; only ECS may receive them.
        Sid      = "PassRoleToEcsTasks"
        Effect   = "Allow"
        Action   = ["iam:PassRole"]
        Resource = [local.ci_arn.execution_role, local.ci_arn.task_role]
        Condition = {
          StringEquals = { "iam:PassedToService" = "ecs-tasks.amazonaws.com" }
        }
      },
      {
        # UpdateFunctionConfiguration passes the balances Lambda its role; only Lambda may receive it.
        Sid      = "PassRoleToBalancesLambda"
        Effect   = "Allow"
        Action   = ["iam:PassRole"]
        Resource = local.ci_arn.balances_lambda_role
        Condition = {
          StringEquals = { "iam:PassedToService" = "lambda.amazonaws.com" }
        }
      },
      {
        # The ALB attribute modify a targeted apply drags in as a dependency of the service.
        Sid    = "AlbModify"
        Effect = "Allow"
        Action = [
          "elasticloadbalancing:ModifyListener",
          "elasticloadbalancing:ModifyListenerAttributes",
          "elasticloadbalancing:ModifyLoadBalancerAttributes",
          "elasticloadbalancing:ModifyTargetGroupAttributes",
        ]
        Resource = [
          "arn:aws:elasticloadbalancing:us-east-2:${local.ci_account}:loadbalancer/app/facilitator-production/*",
          "arn:aws:elasticloadbalancing:us-east-2:${local.ci_account}:listener/app/facilitator-production/*",
          "arn:aws:elasticloadbalancing:us-east-2:${local.ci_account}:targetgroup/facilitator-production*/*",
        ]
      },
      {
        Sid      = "TerraformStateWrite"
        Effect   = "Allow"
        Action   = ["s3:PutObject"]
        Resource = "arn:aws:s3:::${local.ci_state.bucket}/${local.ci_state.key}"
      },
      {
        Sid      = "TerraformLockWrite"
        Effect   = "Allow"
        Action   = ["dynamodb:PutItem", "dynamodb:DeleteItem"]
        Resource = "arn:aws:dynamodb:us-east-2:${local.ci_account}:table/${local.ci_state.table}"
      },
    ]
  })
}

resource "aws_iam_role_policy_attachment" "github_plan_read" {
  role       = aws_iam_role.github_plan.name
  policy_arn = aws_iam_policy.github_read.arn
}

resource "aws_iam_role_policy_attachment" "github_deploy_read" {
  role       = aws_iam_role.github_deploy.name
  policy_arn = aws_iam_policy.github_read.arn
}

resource "aws_iam_role_policy_attachment" "github_deploy_writes" {
  role       = aws_iam_role.github_deploy.name
  policy_arn = aws_iam_policy.github_deploy_writes.arn
}

# DynamoDB, alarms, metric filters, the Lambda code, the task role's inline policies, the cluster
# update: the writes the deploy already needed, unchanged (cicd-iam-policy.tf).
#
# Referenced, not composed like the ARNs inside the policies: an attachment's policy_arn is
# validated as an ARN, and in every tftest whose mock makes up the account a composed one fails the
# teardown -- and the whole `terraform test` step of CI with it (measured 2026-11-18). A pending
# change to cicd_infra would show in the hand apply's plan: stop there if it does.
resource "aws_iam_role_policy_attachment" "github_deploy_infra" {
  role       = aws_iam_role.github_deploy.name
  policy_arn = aws_iam_policy.cicd_infra.arn
}
