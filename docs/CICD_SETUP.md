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
then give it two policies:

**(a) Reads — attach the AWS managed `ReadOnlyAccess` policy.** A full `terraform apply` refreshes the
whole prod config (ECS, ALB, ACM, Route53, DynamoDB, Secrets metadata, CloudWatch, Lambda, …), so the
role needs broad read. `ReadOnlyAccess` covers it without the 2 KB inline-policy size limit:

```bash
aws iam attach-user-policy --user-name github-actions-facilitator-deploy \
  --policy-arn arn:aws:iam::aws:policy/ReadOnlyAccess
```

**(b) Writes + secret-value deny — attach this inline policy** (`facilitator-cicd`). It grants only what
the deploy *writes* (push to ECR, register the task def, roll the service, pass the task roles, and
read/write the Terraform state backend) and explicitly **denies reading secret values** so the CI key
can never exfiltrate production secrets:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "EcrPush",
      "Effect": "Allow",
      "Action": [
        "ecr:GetAuthorizationToken",
        "ecr:BatchCheckLayerAvailability",
        "ecr:GetDownloadUrlForLayer",
        "ecr:BatchGetImage",
        "ecr:InitiateLayerUpload",
        "ecr:UploadLayerPart",
        "ecr:CompleteLayerUpload",
        "ecr:PutImage"
      ],
      "Resource": "*"
    },
    {
      "Sid": "EcsAndElbWrite",
      "Effect": "Allow",
      "Action": [
        "ecs:RegisterTaskDefinition",
        "ecs:DeregisterTaskDefinition",
        "ecs:UpdateService",
        "ecs:TagResource",
        "iam:PassRole"
      ],
      "Resource": "*"
    },
    {
      "Sid": "TerraformStateRW",
      "Effect": "Allow",
      "Action": ["s3:PutObject", "s3:DeleteObject"],
      "Resource": "arn:aws:s3:::facilitator-terraform-state/*"
    },
    {
      "Sid": "TerraformLockRW",
      "Effect": "Allow",
      "Action": ["dynamodb:PutItem", "dynamodb:DeleteItem"],
      "Resource": "arn:aws:dynamodb:us-east-2:518898403364:table/facilitator-terraform-locks"
    },
    {
      "Sid": "DenySecretValueReads",
      "Effect": "Deny",
      "Action": "secretsmanager:GetSecretValue",
      "Resource": "*"
    }
  ]
}
```

```bash
aws iam put-user-policy --user-name github-actions-facilitator-deploy \
  --policy-name facilitator-cicd --policy-document file://ci-policy.json
```

> Reads come from `ReadOnlyAccess` (state read, ECS/ELB/secret-metadata describe, etc.); the inline
> policy adds only the writes (`s3:PutObject`, `dynamodb:PutItem`, `ecs:Register/Update`, `ecr:Put*`,
> `iam:PassRole`). `iam:PassRole` is `Resource: "*"` for simplicity — tighten to the ECS task/execution
> role ARNs (`aws_iam_role.ecs_task` / `aws_iam_role.ecs_task_execution`) to harden.

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
- **Deploy:** a **targeted** `terraform apply -target=aws_ecs_task_definition.facilitator
  -target=aws_ecs_service.facilitator -var image_tag=...`. This scopes the deploy to **only** rolling
  the image. A full apply would additionally re-upload the balances Lambda every run (the
  `archive_file` zip hashes differently in CI than in state) and touch the ALB — neither belongs in
  an image deploy. `-target` avoids the Lambda; the one no-op ALB-attribute modify it spuriously pulls
  in (the service's ALB dependency) is covered by the role's `elasticloadbalancing:Modify*` perms.
  `-refresh=false` is avoided (it invents drift from stale state).
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
