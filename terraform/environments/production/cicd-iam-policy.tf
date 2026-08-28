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
#   terraform import aws_iam_policy.cicd_infra #     arn:aws:iam::518898403364:policy/facilitator-cicd-infra
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
  description = "Infra writes for the GitHub Actions deploy user. Declared in Terraform, applied by hand -- see the comment above."

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
          "arn:aws:dynamodb:us-east-2:518898403364:table/facilitator_transactions",
          "arn:aws:dynamodb:us-east-2:518898403364:table/facilitator-nonces",
          "arn:aws:dynamodb:us-east-2:518898403364:table/idempotency_records"
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
        "Resource" : "arn:aws:iam::518898403364:role/facilitator-production-ecs-task"
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
          "arn:aws:iam::518898403364:role/facilitator-production-ecs-task"
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
        "Resource" : "arn:aws:lambda:us-east-2:518898403364:function:facilitator-production-balances"
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
          "arn:aws:sns:us-east-2:518898403364:facilitator-production-alerts",
          "arn:aws:sns:us-east-2:518898403364:facilitator-production-alerts:*"
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
        "Resource" : "arn:aws:cloudwatch:us-east-2:518898403364:alarm:facilitator-*"
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
        "Resource" : "arn:aws:events:us-east-2:518898403364:rule/facilitator-production-*"
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
        "Resource" : "arn:aws:sqs:us-east-2:518898403364:facilitator-production-alerts-backup"
      },
      {
        "Sid" : "FacilitatorLogRetention",
        "Effect" : "Allow",
        "Action" : [
          "logs:PutRetentionPolicy",
          "logs:DeleteRetentionPolicy",
          "logs:DescribeLogGroups"
        ],
        "Resource" : "arn:aws:logs:us-east-2:518898403364:log-group:/ecs/facilitator-production*"
      }
    ]
  })
}
