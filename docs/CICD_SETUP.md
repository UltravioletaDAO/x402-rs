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
then give it the policies below.

> **A third, managed policy governs the infra writes: `facilitator-cicd-infra`** (account-local,
> not AWS-managed). As of 2026-08-28 it is **declared in Terraform** at
> `terraform/environments/production/cicd-iam-policy.tf` — read that file's header before changing
> anything. It is deliberately absent from CI's `-target` list and **must stay that way**: if CI
> could apply it, CI could grant itself permissions and `DenyPrivilegeEscalation` would be
> decorative. Declaring it is safe only because the deploy user has no `iam:CreatePolicyVersion`.
> Apply it by hand, with a human's credentials, on Terraform 1.9.8.
>
> Its nine statements at `v5`: `DynamoTableManage`, `IamRolePolicyForTaskRoleOnly`,
> `DenyPrivilegeEscalation` (Deny), `BalancesLambdaDeploy`, `AlertTopicManage`,
> `FacilitatorAlarmManage`, `FacilitatorScheduleManage`, `AlertsBackupQueueManage`,
> `FacilitatorLogRetention`.
>
> **Three of those were added only after a deploy failed on their absence** — the pattern this
> document exists to stop:
>
> | When | Missing permission | What it broke |
> |---|---|---|
> | 2026-08-20 | `lambda:UpdateFunctionConfiguration` | The balances Lambda had been silently un-deployable for as long as it existed |
> | 2026-08-28 | `sqs:CreateQueue` | The alerts backup queue |
> | 2026-08-28 | `logs:PutRetentionPolicy` | A log-retention change broke the **routine** deploy for the whole project |
>
> **AWS caps a managed policy at 5 versions.** `v1` was deleted on 2026-08-28 to make room for
> `v5`. Terraform does not rotate them, so an apply that fails with `LimitExceeded` means you must
> delete the oldest non-default version by hand.

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
  an image deploy. The one no-op ALB-attribute modify it spuriously pulls in (the service's ALB
  dependency) is covered by the role's `elasticloadbalancing:Modify*` perms.
  `-refresh=false` is avoided (it invents drift from stale state).
- **Balances Lambda:** a **separate step**, applied only when `lambda/balances/**` or
  `lambda-balances.tf` changed in the push (detected via the compare API). Until 2026-08-20 the
  Lambda was excluded from every run, and the zip-hash reason above was only half the story — the
  deploy user held **no `lambda:*` write permissions at all**, so a full apply could not have
  succeeded either. A change to `lambda/balances/` would land in `main` and never reach AWS: that is
  how `RPC_URL_SUI` stayed pointed at a dead endpoint in the Lambda while the same commit fixed it
  for the facilitator. The `BalancesLambdaDeploy` statement in `facilitator-cicd-infra` (added
  2026-08-20, scoped to that one function ARN) grants the four writes Terraform needs.
  If the compare call fails the step applies anyway — a redundant `UpdateFunctionCode` is cheap, a
  silently unapplied change is the bug being fixed. That fallback doubles as the manual escape
  hatch: a `workflow_dispatch` run has no `github.event.before`, so it always applies the Lambda.
  **Use it to resync the Lambda whenever it drifts from `main`.**
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
