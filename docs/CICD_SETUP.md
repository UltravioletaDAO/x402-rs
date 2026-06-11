# CI/CD Setup — GitHub Actions → AWS ECR → ECS

The `.github/workflows/ci.yaml` pipeline builds + tests on every PR and push to `main`, and
on push to `main` it builds the Docker image, pushes it to **AWS ECR**, runs **`terraform apply`**
to roll ECS production to the new image, and verifies `/health`.

It authenticates to AWS with **IAM access-key secrets**. Until those secrets exist the deploy job
is **skipped** (the run still goes green on the `test` job), so merging the workflow itself is safe.

> Security note: long-lived access keys in GitHub are the quick path you chose. They grant an
> automated actor standing access to production. Rotate them periodically, and consider migrating
> to **GitHub OIDC** (no stored keys) later — see "Hardening" at the bottom.

---

## One-time setup

### 1. Create an IAM user for CI

Create an IAM user (e.g. `github-actions-facilitator-deploy`) with **programmatic access** (no console),
and attach the policy below. It is scoped to exactly what the pipeline needs: push to ECR, roll the
ECS service, pass the task roles, refresh the targeted Terraform resources, and read/write the
Terraform state backend.

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "EcrPushPull",
      "Effect": "Allow",
      "Action": [
        "ecr:GetAuthorizationToken",
        "ecr:BatchCheckLayerAvailability",
        "ecr:BatchGetImage",
        "ecr:GetDownloadUrlForLayer",
        "ecr:InitiateLayerUpload",
        "ecr:UploadLayerPart",
        "ecr:CompleteLayerUpload",
        "ecr:PutImage"
      ],
      "Resource": "*"
    },
    {
      "Sid": "EcsDeploy",
      "Effect": "Allow",
      "Action": [
        "ecs:DescribeClusters",
        "ecs:DescribeServices",
        "ecs:DescribeTaskDefinition",
        "ecs:DescribeTasks",
        "ecs:ListTasks",
        "ecs:RegisterTaskDefinition",
        "ecs:DeregisterTaskDefinition",
        "ecs:UpdateService",
        "ecs:TagResource"
      ],
      "Resource": "*"
    },
    {
      "Sid": "PassTaskRoles",
      "Effect": "Allow",
      "Action": [
        "iam:PassRole",
        "iam:GetRole",
        "iam:ListAttachedRolePolicies",
        "iam:ListRolePolicies",
        "iam:GetRolePolicy"
      ],
      "Resource": "*"
    },
    {
      "Sid": "TerraformRefreshReads",
      "Effect": "Allow",
      "Action": [
        "ec2:Describe*",
        "elasticloadbalancing:Describe*",
        "logs:DescribeLogGroups",
        "logs:DescribeLogStreams",
        "logs:ListTagsForResource",
        "application-autoscaling:Describe*",
        "secretsmanager:DescribeSecret",
        "secretsmanager:GetResourcePolicy",
        "secretsmanager:ListSecretVersionIds",
        "servicediscovery:Get*",
        "servicediscovery:List*",
        "acm:DescribeCertificate",
        "acm:ListTagsForCertificate",
        "dynamodb:DescribeTable",
        "dynamodb:DescribeContinuousBackups",
        "dynamodb:DescribeTimeToLive",
        "dynamodb:ListTagsOfResource",
        "route53:ListHostedZones",
        "route53:GetHostedZone",
        "route53:ListResourceRecordSets",
        "route53:ListTagsForResource",
        "route53:GetChange",
        "kms:DescribeKey"
      ],
      "Resource": "*"
    },
    {
      "Sid": "TerraformState",
      "Effect": "Allow",
      "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket"],
      "Resource": [
        "arn:aws:s3:::facilitator-terraform-state",
        "arn:aws:s3:::facilitator-terraform-state/*"
      ]
    },
    {
      "Sid": "TerraformLock",
      "Effect": "Allow",
      "Action": ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:DeleteItem"],
      "Resource": "arn:aws:dynamodb:us-east-2:518898403364:table/facilitator-terraform-locks"
    }
  ]
}
```

> `iam:PassRole` is `Resource: "*"` for simplicity. To tighten it, replace `*` with the exact ARNs of
> the ECS **task** and **execution** roles (`aws_iam_role.ecs_task` / `aws_iam_role.ecs_task_execution`
> in `terraform/environments/production/main.tf`).

Then create an access key for that user and copy the **Access key ID** and **Secret access key**.

### 2. Add the secrets to GitHub

Set them as **repository secrets** on `UltravioletaDAO/x402-rs` (Settings → Secrets and variables →
Actions), or via CLI:

```bash
gh secret set AWS_ACCESS_KEY_ID     --repo UltravioletaDAO/x402-rs --body 'AKIA...'
gh secret set AWS_SECRET_ACCESS_KEY --repo UltravioletaDAO/x402-rs --body 'YOUR_SECRET_KEY'
```

That's it. The next push to `main` will build → push to ECR → `terraform apply` → verify automatically.

---

## How it works

| Trigger | `test` job | `deploy` job |
|---|---|---|
| Pull request → `main` | ✅ build + full test suite | skipped |
| Push → `main` (no AWS secrets) | ✅ | **skipped** (run stays green) |
| Push → `main` (secrets set) | ✅ | ✅ build → ECR → terraform apply → verify |

- **Image tag:** `<Cargo.toml version>-<short-sha>` (e.g. `1.47.0-6999058`) plus `:latest`, pushed to
  `518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator`.
- **Deploy:** a **targeted** `terraform apply` of only `aws_ecs_task_definition.facilitator` and
  `aws_ecs_service.facilitator`, overriding `image_tag` via `-var`. This rolls the service to the new
  image without touching the rest of the infrastructure — the same change my manual deploys make.
- **Verify:** waits for `services-stable`, then polls `/health` for `200`.
- `concurrency: deploy-production` serializes deploys so two merges can't apply at once.

> Because CI overrides `image_tag` via `-var`, the value committed in `terraform.tfvars` becomes a
> non-authoritative default — the pipeline is the source of truth for what's deployed. Bump the
> `Cargo.toml` version for human-readable release tags; the SHA suffix keeps every build unique.

---

## Manual deploy (still available)

The local path is unchanged and works as a fallback:

```bash
./scripts/fast-build.sh <version> --push
cd terraform/environments/production
terraform apply -var="image_tag=<version>"
```

---

## Hardening (recommended follow-ups)

1. **Migrate to GitHub OIDC** — drop the long-lived keys entirely: add an
   `aws_iam_openid_connect_provider` for `token.actions.githubusercontent.com` and an IAM role whose
   trust policy is scoped to `repo:UltravioletaDAO/x402-rs:ref:refs/heads/main`, then swap the
   `configure-aws-credentials` step to `role-to-assume`. No secrets in GitHub.
2. **Require a manual gate for prod** — wrap the `deploy` job in a GitHub **Environment**
   (`production`) with required reviewers if you later want a human approval before each apply.
3. **Scope `iam:PassRole`** to the two ECS role ARNs (above).
4. **Rotate** the access keys on a schedule until OIDC lands.
