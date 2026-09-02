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
      "Resource": "arn:aws:dynamodb:us-east-2:<AWS_ACCOUNT_ID>:table/facilitator-terraform-locks"
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
  `<AWS_ACCOUNT_ID>.dkr.ecr.us-east-2.amazonaws.com/facilitator`.
- **Deploy:** a **targeted** `terraform apply -target=aws_ecs_task_definition.facilitator
  -target=aws_ecs_service.facilitator -target=aws_appautoscaling_target.ecs_target
  -target=aws_appautoscaling_policy.ecs_alb_request_count -target=aws_appautoscaling_policy.ecs_memory
  -var image_tag=...`. This scopes the deploy to **only** rolling the image (plus autoscaling — see
  below for why those three ride along). A full apply would additionally re-upload the balances
  Lambda every run (the `archive_file` zip hashes differently in CI than in state) and touch the
  ALB — neither belongs in an image deploy. The one no-op ALB-attribute modify it spuriously pulls
  in (the service's ALB dependency) is covered by the role's `elasticloadbalancing:Modify*` perms.
  `-refresh=false` is avoided (it invents drift from stale state).
- **Autoscaling rides in the same `-target` set, and that is deliberate — see "The `-target`
  dependency trap" below before removing or moving any of the three.** They were added
  2026-08-29 after `aws_appautoscaling_policy.ecs_alb_request_count` sat declared in `main.tf` for
  a day, unapplied by any CI run, while the service ran on a CPU-target-tracking policy alone
  (75% CPU on an I/O-bound service that measured 1-2% CPU in three separate degradation episodes)
  during a 3x traffic spike. It never scaled.
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

## The `-target` dependency trap

`terraform apply -target=X` does not just apply `X` — it walks `X`'s full dependency chain and
**applies every pending change on every resource in that chain**, not merely the ones needed to
resolve `X`'s own attributes. This is documented AWS/Terraform behavior (the CLI prints a warning
about it every time), but it is easy to reason past, because a plain string interpolation still
counts as a dependency edge even when the referenced attribute obviously doesn't need the
dependency's *other* pending changes.

**Concrete case, 2026-08-29.** `aws_appautoscaling_target.ecs_target` in
`terraform/environments/production/main.tf` has:

```hcl
resource_id = "service/${aws_ecs_cluster.main.name}/${aws_ecs_service.facilitator.name}"
```

`.name` on both sides is static — it was never going to change. But that interpolation is still a
reference to `aws_ecs_service.facilitator`, which is itself a reference to
`aws_ecs_task_definition.facilitator` (via its `task_definition` argument). At the time, the task
definition had unrelated pending drift: `RUST_LOG` was still `info,x402_rs::chain::evm=debug` in
the deployed revision (a debug window from `68860bbe` that was supposed to have been turned back
off by `ff781321`, but never reached AWS because it wasn't in CI's `-target` list either — the
same failure mode, one layer down).

Running `terraform plan -target=aws_appautoscaling_target.ecs_target` (or targeting either of its
policies, which reference `ecs_target.resource_id`) pulled in `aws_ecs_service.facilitator` **and**
`aws_ecs_task_definition.facilitator` with a `-/+ replace`, forcing a brand-new task definition
revision and an ECS rollout — a production deploy nobody asked for, as a side effect of an
autoscaling fix. Confirmed twice, with the actual plan output, before anything was applied.

**When this is safe to ignore:** if `terraform plan -target=<your resource>` comes back showing
*only* your resource (or resources with no pending diff), applying with the same `-target` is fine
— that's the routine case, and it's what the deploy step above does every run. **Read the plan
first, every time**, specifically for resources you didn't expect to see.

**When it isn't, the recipe used here was:**

1. Make the change directly via AWS CLI (`aws application-autoscaling put-scaling-policy` /
   `delete-scaling-policy` / `register-scalable-target`), matching the config already declared in
   `.tf` exactly, so the two don't disagree afterward.
2. Reconcile Terraform's state to match, **without ever calling `apply` on the resources you're
   avoiding**:
   - New resource: `terraform import <address> <id>` (for `aws_appautoscaling_policy`, the ID is
     `<service-namespace>/<resource-id>/<scalable-dimension>/<policy-name>`).
   - Resource removed outside Terraform: `terraform state rm <address>`.
   - Existing resource with a changed argument (e.g. `min_capacity`): `terraform apply
     -refresh-only -target=<address>` — this only reads reality into state, it never proposes or
     applies changes to anything, so it cannot cascade into unrelated resources the way a normal
     `-target` apply does.
3. Verify with `terraform plan -target=<the resources you touched>` — expect `No changes`. If it
   still shows something, state and reality disagree; don't force it, find out why.

This is **more fragile than a normal `-target` apply** — CLI calls and Terraform state can drift
apart if a step is missed or fails halfway, and it's manual per-resource work instead of one
command. Only reach for it when a `plan` first showed you the routine path would drag in something
you did not intend to deploy. Verify against AWS directly afterward
(`describe-scaling-policies` / `describe-scalable-targets`, not the `apply` output) — the same
discipline as the rest of this repo's "verify before reporting state" rule.

### Current autoscaling config (2026-08-29)

| Resource | Config | Notes |
|---|---|---|
| `aws_appautoscaling_target.ecs_target` | `min_capacity=2`, `max_capacity=3` | Floor of 2 is the resilience minimum — one task alone eats any replacement or spike solo. **Registering a new `min_capacity` does not itself move `desired_count` up** — Application Auto Scaling only enforces the floor the next time a scaling policy actually evaluates (or a manual `set-desired-capacity`). Measured here: `min_capacity` was already 2 by the time it was checked at 14:34 UTC, but `RunningTaskCount` stayed flat at 1 through 14:21 UTC and only started climbing at 14:26 UTC (reaching 3 by 14:31), when the ALB-request-count policy's own alarm fired off real traffic — not the floor registration by itself. |
| `aws_appautoscaling_policy.ecs_alb_request_count` | `ALBRequestCountPerTarget`, target `15` req/min/target | Replaced a CPU-target-tracking policy that never fired (service is I/O-bound; CPU measured 1-2% across three degradation episodes). Baseline traffic (~900-1200 req/h over 2 tasks) sits comfortably under target (~7.5-10 req/min/target); a 3x incident like 2026-08-29 (~2600-3300 req/h) pushes it to ~22-27 req/min/target, which the target-tracking formula (`desired = ceil(current × metric/target)`) resolves to the `max_capacity` ceiling on the first evaluation. |
| `aws_appautoscaling_policy.ecs_memory` | `ECSServiceAverageMemoryUtilization`, target `80%` | Unchanged, kept as an independent signal — a memory leak doesn't show up in request count. |
| ~~`aws_appautoscaling_policy.ecs_cpu`~~ | *(removed 2026-08-29)* | 75% CPU target on a service that never measured above 25%. Multiple target-tracking policies can coexist on one target without conflict (scale-out honors whichever policy asks for the most capacity; scale-in only happens once all of them agree), so this wasn't removed for safety — it was dead weight, one more CloudWatch alarm pair to reason about in an on-call with zero expected benefit. Cheap to reintroduce (~5 lines) if a genuinely CPU-bound regression shows up later. |

**`max_capacity=3` is not the bottleneck and should not be raised on reflex.** During the
2026-08-29 incident, scaling to 3 tasks did **not** bring p99 down (stayed at ~11.6s) — the real
cause was `/identity/owner` making ~20 sequential `eth_call`s to count agents where the contract's
`totalSupply()` answers in one call; three tasks were just running the same 22 round-trips each, in
parallel. **Do not raise `max_capacity` until that fix ships and the ceiling is actually observed
being hit again with real numbers** — raising it now would pay for capacity that doesn't address
the actual constraint.

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
