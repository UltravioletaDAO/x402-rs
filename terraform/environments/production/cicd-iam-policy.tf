# ============================================================================
# The CI deploy user's IAM policy -- DECLARED here, APPLIED by hand
# ============================================================================
#
# This policy governs what `github-actions-facilitator-deploy` may write. It
# lived ONLY in AWS and in docs/CICD_SETUP.md until 2026-08-28, and that is
# exactly what made three separate missing permissions invisible until a deploy
# failed on them:
#
#   2026-08-20  lambda:UpdateFunctionConfiguration  -> the balances Lambda had
#               been silently un-deployable for as long as it existed
#   2026-08-28  sqs:CreateQueue                     -> the alerts backup queue
#   2026-08-28  logs:PutRetentionPolicy             -> a log-retention change
#               broke the ROUTINE deploy for the whole project
#   2026-08-29  logs:PutRetentionPolicy (SCOPE, not grant) -> the Resource covered
#               only /ecs/facilitator-production*, so the balances Lambda and API
#               Gateway log groups drifted to 7d against the code's 30 and nothing
#               -- not CI, not a routine deploy -- could ever fix them
#   2026-09-10  logs:PutMetricFilter                -> the deploy could create the
#               ALARM but not the log metric filter it reads, so every log-based
#               alarm this stack owns was un-deployable. See below.
#
# There is no drift detection for a resource that is not declared anywhere.
#
# ---------------------------------------------------------------------------
# WHY THIS IS NOT IN CI'S -target LIST, AND MUST NEVER BE
# ---------------------------------------------------------------------------
#
# If CI could apply this, CI could grant itself permissions, and the
# DenyPrivilegeEscalation statement below would be decorative.
#
# Declaring it here is SAFE precisely because CI cannot apply it: the deploy
# user has no iam:CreatePolicyVersion (its own Deny blocks it). So this is a
# versioned, reviewable record that the pipeline is structurally unable to act
# on -- read-only from CI's side, by construction.
#
# Apply it by hand, with a human's credentials, from a machine running
# Terraform 1.9.8 (see docs/CICD_SETUP.md for why the version matters):
#
#   terraform apply -target=aws_iam_policy.cicd_infra
#
# ---------------------------------------------------------------------------
# IMPORT (this resource already exists in AWS -- do NOT let Terraform create it)
# ---------------------------------------------------------------------------
#
#   terraform import aws_iam_policy.cicd_infra #     arn:aws:iam::<AWS_ACCOUNT_ID>:policy/facilitator-cicd-infra
#
# The body below is byte-for-byte the live v5 document, so the plan right after
# the import is empty. If it is not empty, someone changed the policy by hand
# after this was written -- read the diff before applying, do not clobber it.
#
# NOTE: AWS caps a managed policy at 5 versions. v1 was deleted on 2026-08-28 to
# make room for v5; Terraform does not manage that rotation, so if an apply ever
# fails with LimitExceeded, delete the oldest non-default version by hand.

resource "aws_iam_policy" "cicd_infra" {
  name        = "facilitator-cicd-infra"
  # The description is a ForceNew attribute on aws_iam_policy: changing it REPLACES the policy,
  # and this one is attached to the CI deploy identity. Keep it byte-identical to the live value
  # (imported into state on 2026-09-02) so a plan never asks to replace it; the statements below
  # are what the plan updates in place.
  description = "DynamoDB table management + scoped IAM for the facilitator task role. Separate from the inline policy because that one is at its 2048-byte limit."

  policy = jsonencode({
    "Version" : "2012-10-17",
    "Statement" : [
      {
        "Sid" : "DynamoTableManage",
        "Effect" : "Allow",
        "Action" : [
          "dynamodb:CreateTable",
          "dynamodb:UpdateTable",
          "dynamodb:DescribeTable",
          "dynamodb:TagResource",
          "dynamodb:UntagResource",
          "dynamodb:ListTagsOfResource",
          "dynamodb:UpdateTimeToLive",
          "dynamodb:DescribeTimeToLive",
          "dynamodb:UpdateContinuousBackups",
          "dynamodb:DescribeContinuousBackups"
        ],
        "Resource" : [
          "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/facilitator_transactions",
          "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/facilitator-nonces",
          "arn:aws:dynamodb:us-east-2:${data.aws_caller_identity.current.account_id}:table/idempotency_records"
        ]
      },
      {
        "Sid" : "IamRolePolicyForTaskRoleOnly",
        "Effect" : "Allow",
        "Action" : [
          "iam:PutRolePolicy",
          "iam:GetRolePolicy",
          "iam:DeleteRolePolicy",
          "iam:ListRolePolicies"
        ],
        "Resource" : "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/facilitator-production-ecs-task"
      },
      {
        "Sid" : "DenyPrivilegeEscalation",
        "Effect" : "Deny",
        "Action" : [
          "iam:PutRolePolicy",
          "iam:AttachRolePolicy",
          "iam:PutUserPolicy",
          "iam:AttachUserPolicy",
          "iam:CreateRole",
          "iam:CreateUser",
          "iam:CreateAccessKey",
          "iam:CreatePolicy",
          "iam:CreatePolicyVersion",
          "iam:UpdateAssumeRolePolicy"
        ],
        "NotResource" : [
          "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/facilitator-production-ecs-task"
        ]
      },
      {
        "Sid" : "BalancesLambdaDeploy",
        "Effect" : "Allow",
        "Action" : [
          "lambda:UpdateFunctionConfiguration",
          "lambda:UpdateFunctionCode",
          "lambda:TagResource",
          "lambda:UntagResource",
          "lambda:AddPermission",
          "lambda:RemovePermission"
        ],
        "Resource" : "arn:aws:lambda:us-east-2:${data.aws_caller_identity.current.account_id}:function:facilitator-production-balances"
      },
      {
        "Sid" : "AlertTopicManage",
        "Effect" : "Allow",
        "Action" : [
          "sns:CreateTopic",
          "sns:DeleteTopic",
          "sns:GetTopicAttributes",
          "sns:SetTopicAttributes",
          "sns:TagResource",
          "sns:UntagResource",
          "sns:ListTagsForResource",
          "sns:Subscribe",
          "sns:Unsubscribe",
          "sns:GetSubscriptionAttributes",
          "sns:SetSubscriptionAttributes"
        ],
        "Resource" : [
          "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:facilitator-production-alerts",
          "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:facilitator-production-alerts:*"
        ]
      },
      {
        "Sid" : "FacilitatorAlarmManage",
        "Effect" : "Allow",
        "Action" : [
          "cloudwatch:PutMetricAlarm",
          "cloudwatch:DeleteAlarms",
          "cloudwatch:TagResource",
          "cloudwatch:UntagResource",
          "cloudwatch:ListTagsForResource"
        ],
        "Resource" : "arn:aws:cloudwatch:us-east-2:${data.aws_caller_identity.current.account_id}:alarm:facilitator-*"
      },
      {
        # Added 2026-09-10. `FacilitatorAlarmManage` above grants PutMetricAlarm,
        # so the deploy could always create the ALARM -- but every log-based alarm
        # this stack owns reads a metric that only exists because of a
        # `aws_cloudwatch_log_metric_filter`, and that call was never granted. The
        # two halves are one feature and the policy only had one of them.
        #
        # It stayed invisible because of a second bug, fixed in the same push
        # (#34): the "Deploy observability" step's change gate listed
        # `alerts.tf|alerts-imported.tf|cloudwatch-*.tf|variables.tf` by name, which
        # silently excluded `alerts-solana-mint.tf`. Its two resources -- a filter
        # and an alarm, added 2026-09-10 -- had therefore never been applied, so
        # the missing permission had never been exercised. Widening the gate to
        # `alerts-.*\.tf` made the deploy actually try, and it failed:
        #
        #   Error: putting CloudWatch Logs Metric Filter (): api error
        #   AccessDeniedException: User: .../github-actions-facilitator-deploy is
        #   not authorized to perform: logs:PutMetricFilter on resource:
        #   arn:aws:logs:us-east-2:...:log-group:/ecs/facilitator-production
        #   because no identity-based policy allows the logs:PutMetricFilter action
        #
        # (run 34500930933, 2026-09-10 16:27:35Z. The ECS rollout to 2.19.0 was a
        # separate, earlier step and completed fine; only this step failed.)
        #
        # Both ARN forms are listed explicitly rather than with a trailing `*`. The
        # API reports the resource WITHOUT the `:*` suffix, as the error above
        # shows, while other log-group calls use the suffixed form -- and a
        # trailing wildcard would also match `/ecs/facilitator-production-anything`,
        # which is wider than this needs to be.
        #
        # DescribeMetricFilters is here because it is Terraform's READ: without it
        # a plan cannot tell an existing filter from a missing one.
        #
        # Applied out of band by c0der, 2026-09-10 16:40Z (plan 0/1/0; simulating
        # logs:PutMetricFilter and logs:DeleteMetricFilter now returns allowed).
        "Sid" : "FacilitatorMetricFilterManage",
        "Effect" : "Allow",
        "Action" : [
          "logs:PutMetricFilter",
          "logs:DeleteMetricFilter",
          "logs:DescribeMetricFilters"
        ],
        "Resource" : [
          "arn:aws:logs:us-east-2:${data.aws_caller_identity.current.account_id}:log-group:/ecs/facilitator-production",
          "arn:aws:logs:us-east-2:${data.aws_caller_identity.current.account_id}:log-group:/ecs/facilitator-production:*"
        ]
      },
      {
        "Sid" : "FacilitatorScheduleManage",
        "Effect" : "Allow",
        "Action" : [
          "events:PutRule",
          "events:DeleteRule",
          "events:PutTargets",
          "events:RemoveTargets",
          "events:TagResource",
          "events:UntagResource",
          "events:ListTagsForResource"
        ],
        "Resource" : "arn:aws:events:us-east-2:${data.aws_caller_identity.current.account_id}:rule/facilitator-production-*"
      },
      {
        "Sid" : "AlertsBackupQueueManage",
        "Effect" : "Allow",
        "Action" : [
          "sqs:CreateQueue",
          "sqs:SetQueueAttributes",
          "sqs:GetQueueAttributes",
          "sqs:GetQueueUrl",
          "sqs:TagQueue",
          "sqs:UntagQueue",
          "sqs:ListQueueTags"
        ],
        "Resource" : "arn:aws:sqs:us-east-2:${data.aws_caller_identity.current.account_id}:facilitator-production-alerts-backup"
      },
      {
        # Scope widened 2026-08-29: only /ecs/facilitator-production* was covered here,
        # so /aws/lambda/facilitator-production-balances and
        # /aws/apigateway/facilitator-production-balances (both declared with
        # retention_in_days = var.log_retention_days in lambda-balances.tf) drifted to 7
        # days against the code's 30 and NOTHING could fix it -- not CI, not a routine
        # deploy, because this is the only grant of logs:PutRetentionPolicy the deploy
        # user has. Drift audit, 2026-08-29. NOT applied here -- IAM change, goes through
        # Saul first.
        "Sid" : "FacilitatorLogRetention",
        "Effect" : "Allow",
        "Action" : [
          "logs:PutRetentionPolicy",
          "logs:DeleteRetentionPolicy",
          "logs:DescribeLogGroups"
        ],
        "Resource" : [
          "arn:aws:logs:us-east-2:${data.aws_caller_identity.current.account_id}:log-group:/ecs/facilitator-production*",
          "arn:aws:logs:us-east-2:${data.aws_caller_identity.current.account_id}:log-group:/aws/lambda/facilitator-production-balances*",
          "arn:aws:logs:us-east-2:${data.aws_caller_identity.current.account_id}:log-group:/aws/apigateway/facilitator-production-balances*"
        ]
      },
      {
        # Added 2026-09-10 for terraform/environments/production/discovery-bucket.tf.
        #
        # The Bazaar catalog bucket was created by hand and has never been versioned, so
        # every registration -- a read-modify-write of one 15 MB object -- left the previous
        # catalog with no copy behind it. Turning versioning on is a deploy step now, and
        # the deploy user could not do it: simulated against the live identity on
        # 2026-09-10, s3:PutBucketVersioning and s3:PutLifecycleConfiguration both came back
        # implicitDeny. The two reads are already granted by the attached ReadOnlyAccess and
        # are named here anyway, so this statement stands on its own if that attachment ever
        # goes away -- the drift gate's plan needs them to refresh the resource.
        #
        # Scoped to the one bucket. Not `s3:*`, and not a prefix: the deploy user has no
        # business reconfiguring the Terraform state bucket, the ALB log bucket or the DX402
        # evidence bucket, and this is a bucket-configuration grant, not an object grant --
        # it cannot read or write a single byte of the catalog.
        "Sid" : "DiscoveryBucketVersioning",
        "Effect" : "Allow",
        "Action" : [
          "s3:PutBucketVersioning",
          "s3:GetBucketVersioning",
          "s3:PutLifecycleConfiguration",
          "s3:GetLifecycleConfiguration"
        ],
        "Resource" : "arn:aws:s3:::facilitator-discovery-prod"
      }
    ]
  })
}
