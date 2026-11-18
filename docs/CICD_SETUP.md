# CI/CD Setup — GitHub Actions → AWS ECR → ECS

The `.github/workflows/ci.yaml` pipeline builds + tests on every PR and push to `main`
**that touches a build input**. It deploys **only on a manual "Run workflow" on `main`**
(`workflow_dispatch`, or `gh workflow run ci.yaml --ref main`): it builds the Docker image,
pushes it to **AWS ECR**, runs **`terraform apply`** to roll ECS production to the new image,
and verifies `/health`. **A merge never deploys by itself** — the owner's rule of 2026-09-27
(one release a day, started by hand), in the deploy job's `if:` since X402-CI-GATES and held by
`tests/scripts/test_ci_deploy_job.py`. Before that, every push to `main` that touched a build
input shipped on its own.

A push whose whole diff is documentation triggers nothing: the workflow carries a `paths` list
holding the union of what its four jobs read. `docs/handoffs/2026-09-11-ci-paths.md` has the
job-by-job table and `python3 scripts/ci_paths_selftest.py` asserts the list is neither too
wide nor too narrow. `workflow_dispatch` has no `paths` and deploys whatever `main` holds.

It authenticates to AWS with **GitHub OIDC** (since X402-SEG-CI, 2026-11-18): no key is stored. The
drift gate (`plan`) assumes **`facilitator-github-plan`**, read-only, trusted for pull requests and
`main`; the release (`deploy`) assumes **`facilitator-github-deploy`**, trusted for the **`production`
environment** alone. Both are declared in `terraform/environments/production/cicd-oidc.tf` and applied
by hand. The only repo secret is `AWS_ACCOUNT_ID` (to name the roles; a secret so GitHub masks it in
the public logs). Until it exists **every run but a fork's pull request fails in `preflight`** (the
drift gate and the deploy would otherwise be skipped and the run would go green with nothing checked
or shipped): see "Until steps 1-3 are done, this CI is red on purpose" below. Every action is pinned
to a commit SHA (`tests/scripts/test_ci_actions_pinned.py`). Every workflow's token defaults to
`contents: read`; only `plan` and `deploy` raise theirs (`id-token: write`, `actions: read`).

### Switching on the OIDC roles (once, in this order, all before the merge)

The old `ci.yaml` keeps using the access key until the change merges. **Push the pull request only
after steps 1-3**: without `AWS_ACCOUNT_ID` every run but a fork's pull request fails in `preflight`
(it would otherwise go green with the drift gate and the deploy skipped), and with it the pull
request's own run proves the plan role and runs the step "The production environment admits main
alone", which is red while the environment is missing or admits any branch but `main`, and warns
while it has no required reviewer or `main` is unprotected.

1. On GitHub, **first**: the environment **`production`** with *Deployment branches and tags* = `main`
   only and a **required reviewer**, and `main` protected (no direct push, no force-push). The branch
   rule alone does not stop anyone with write access while `main` is open (measured 2026-11-18: no
   protection, no ruleset): they push a workflow to `main` and get the deploy role's token. And a job
   that names an environment that does not exist yet creates it with no rule at all.
2. Apply the roles by hand, with a human's credentials, on Terraform 1.9.8 — the command is in the
   header of `cicd-oidc.tf`. Read the plan: it must create 2 roles, 2 policies and 4 attachments,
   plus the task role's permissions boundary (`aws_iam_policy.ecs_task_boundary`), and update in
   place `aws_iam_role.ecs_task` (the boundary) and `aws_iam_policy.cicd_infra` (the grant that
   depends on it) — **9 to add, 2 to change, 0 to destroy** — and touch nothing else (the policies
   compose their ARNs from names, so the balances Lambda is not dragged in). The checks before it, the
   plan to expect line by line and the checks after it: "The hand apply (step 2), in detail" below.
3. Set the repository secret `AWS_ACCOUNT_ID`.
4. Merge. After one green push run (drift gate on the plan role) and one green release (deploy role),
   deactivate and delete the access key of `github-actions-facilitator-deploy`, delete the secrets
   `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, then the user. Until then the key is still readable
   by any workflow on any branch: deactivate it on the first green run, not at the next release
   ("After the merge" below has the two moves).
5. Optional, later: opt the repository in to the immutable OIDC subject
   (`use_immutable_subject: true` on `PUT /repos/UltravioletaDAO/x402-rs/actions/oidc/customization/sub`)
   and apply with `-var github_oidc_immutable_subject=true` right after, so the trust names the
   repository by its ids and survives a rename. Between the two the roles refuse every token: do it
   with no run in flight.

#### Until steps 1-3 are done, this CI is red on purpose

Merging this change before the switch-on stops every release, and nothing deploys until it is done:

| Missing | What goes red |
|---|---|
| Step 3 (`AWS_ACCOUNT_ID`) | `preflight`, on every push, every pull request from this repository and every "Run workflow". `plan` and `deploy` do not start. A pull request from a fork passes, its drift gate skipped. |
| Step 2 (the apply) | `plan` and `deploy` at "Configure AWS credentials": the role does not exist, so the token is refused. |
| Step 1 (the environment) | `plan` at "The production environment admits main alone". `deploy` does not wait for `plan` (it needs `test` and `preflight`): a release started now would run, and if the environment does not exist GitHub creates it with no rule when the job starts -- which the step then reports red on every run until the rule is set. |

What the step "The production environment admits main alone" reads (read-only, with the job's own
token, `actions: read`):

| Read | Red | Warning, the run passes |
|---|---|---|
| `GET repos/UltravioletaDAO/x402-rs/environments/production` | 404: no environment | any other failure to read ("inconclusive") |
| its `deployment_branch_policy.custom_branch_policies` | anything but `true` (no rule, or "protected branches") | |
| `GET .../environments/production/deployment-branch-policies` | anything but exactly one rule, of type `branch`, named `main` | a failure to read ("inconclusive") |
| its `protection_rules` | | no `required_reviewers` rule |
| `GET repos/UltravioletaDAO/x402-rs/branches/main` | | `protected` is not `true` |

So the minimum on GitHub is the environment `production` with *Deployment branches and tags* =
*Selected branches and tags* and one branch rule, `main`. A required reviewer and the protection of
`main` are the owner's call; without them the step warns and passes.

#### The hand apply (step 2), in detail

All of it from a checkout of `main` with this change (LF line endings, on a native filesystem), with a
human's administrator credentials, on Terraform 1.9.8. Nothing here is run by CI, and CI cannot run it.

Before:

```bash
# The account's GitHub OIDC provider exists (another stack owns it; cicd-oidc.tf only names it) and
# accepts the audience the roles require. Missing: stop -- creating it is not a step of this runbook.
ACCT=$(aws sts get-caller-identity --query Account --output text)
aws iam get-open-id-connect-provider \
  --open-id-connect-provider-arn "arn:aws:iam::${ACCT}:oidc-provider/token.actions.githubusercontent.com" \
  --query ClientIDList   # must list "sts.amazonaws.com"

# The repository's OIDC subject template is the default one, or every sub the roles trust is wrong
# (step 5 changes it on purpose, later).
gh api repos/UltravioletaDAO/x402-rs/actions/oidc/customization/sub   # "use_default": true

# facilitator-cicd-infra gets a new version, and AWS keeps five: the provider deletes the oldest
# non-default one without showing it in the plan. Keep the current one as a rollback copy.
P="arn:aws:iam::${ACCT}:policy/facilitator-cicd-infra"
V=$(aws iam get-policy --policy-arn "$P" --query Policy.DefaultVersionId --output text)
aws iam get-policy-version --policy-arn "$P" --version-id "$V" > "/dev/shm/cicd-infra-$V.json"

# What the user's inline write policy grants that the deploy role would not. Every write action in
# it that facilitator-github-deploy-writes and facilitator-cicd-infra lack is either added (a pull
# request, applied by hand) or shown unused by the user's recent events.
aws iam get-user-policy --user-name github-actions-facilitator-deploy --policy-name facilitator-cicd \
  --query PolicyDocument > /dev/shm/user-inline.json
aws cloudtrail lookup-events --region us-east-2 --max-results 50 \
  --lookup-attributes AttributeKey=Username,AttributeValue=github-actions-facilitator-deploy \
  --query 'Events[].EventName' --output text | tr '\t' '\n' | sort | uniq -c
```

Plan, with the `-target` list of the header of `cicd-oidc.tf`:

```bash
cd terraform/environments/production
terraform version            # Terraform v1.9.8
terraform init -input=false
terraform plan -input=false -out=oidc.tfplan \
  -target=aws_iam_role.github_plan -target=aws_iam_role.github_deploy \
  -target=aws_iam_policy.github_read -target=aws_iam_policy.github_deploy_writes \
  -target=aws_iam_role_policy_attachment.github_plan_read \
  -target=aws_iam_role_policy_attachment.github_deploy_read \
  -target=aws_iam_role_policy_attachment.github_deploy_writes \
  -target=aws_iam_role_policy_attachment.github_deploy_infra \
  -target=aws_iam_policy.ecs_task_boundary -target=aws_iam_role.ecs_task \
  -target=aws_iam_policy.cicd_infra
```

Compare the plan with this list and stop on any difference:

| Address | Expected |
|---|---|
| `aws_iam_role.github_plan`, `aws_iam_role.github_deploy` | create |
| `aws_iam_policy.github_read`, `aws_iam_policy.github_deploy_writes` | create |
| `aws_iam_role_policy_attachment.github_plan_read`, `.github_deploy_read`, `.github_deploy_writes`, `.github_deploy_infra` | create (4) |
| `aws_iam_policy.ecs_task_boundary` | create |
| `aws_iam_role.ecs_task` | update in place: `permissions_boundary` added, nothing else |
| `aws_iam_policy.cicd_infra` | update in place, `policy` only (`IamRolePolicyReadForTaskRole` and `DenyBoundaryChanges` added, a `Condition` on `IamRolePolicyForTaskRoleOnly`). Never a replacement: its `description` does not change |

Plan summary: **9 to add, 2 to change, 0 to destroy.** The boundary names the facilitator's DynamoDB
tables and the DX402 bucket, and the read policy the Bazaar token's KMS key, the balances API and the
zone: a pending change on any of them would ride along, and there should be none. The task definition,
the service and the two task-role policies in the image deploy's target list
(`hedera_settlement_access`, `ip_allowlist_read`) must not appear at all: they name the role by its
composed name and ARN, the same strings as before. A change to `aws_ecs_task_definition.facilitator` or
to either policy's `role` means the composed ARN is not the live one: stop.

Apply and verify:

```bash
terraform apply oidc.tfplan
aws iam get-role --role-name facilitator-production-ecs-task --query Role.PermissionsBoundary
aws iam simulate-principal-policy --policy-source-arn "arn:aws:iam::${ACCT}:role/facilitator-github-plan" \
  --action-names ssm:GetParameter secretsmanager:GetSecretValue iam:PassRole ecs:UpdateService \
  --resource-arns '*' --query 'EvaluationResults[].[EvalActionName,EvalDecision]' --output text
# explicitDeny x2 (the parameter and the secret), implicitDeny x2: the plan role writes nothing
aws iam simulate-principal-policy --policy-source-arn "arn:aws:iam::${ACCT}:role/facilitator-github-deploy" \
  --action-names ssm:GetParameter secretsmanager:GetSecretValue \
  --resource-arns '*' --query 'EvaluationResults[].[EvalActionName,EvalDecision]' --output text
# explicitDeny x2: the deploy role reads no secret either
```

The boundary takes effect on the running tasks at once (IAM is evaluated on every call). It is wider
than the role's inline policies, so nothing should change; confirm it over the next minutes:

```bash
S=$(( ($(date +%s) - 900) * 1000 ))
aws logs filter-log-events --log-group-name /ecs/facilitator-production --region us-east-2 \
  --start-time $S --filter-pattern AccessDenied --query 'length(events)' --output text
# 0 -- the CLI prints one number per page: sum them. Also /health/ready, GET /api/stats (the
# transactions table), GET /discovery/resources (the discovery bucket), and the receipt-store alarms.
```

If the task is refused anything, lift the boundary at once and revert it in a pull request together
with the condition in `cicd-iam-policy.tf` (applied by hand), or the deploy loses the task role's
inline-policy writes:

```bash
aws iam delete-role-permissions-boundary --role-name facilitator-production-ecs-task
```

#### After the merge: verify, roll back, retire the key

- **The pull request and the push to `main`**: "Terraform plan (drift gate)" assumes
  `facilitator-github-plan` (session `x402-rs-plan-<run id>`), the plan completes, and "The production
  environment admits main alone" passes.
- **The first release**: "Configure AWS credentials" in `deploy` assumes `facilitator-github-deploy`
  (session `x402-rs-deploy-<run id>`), the image is pushed, the targeted applies, the rollout wait and
  `/health` pass. Compare `curl -s https://facilitator.ultravioletadao.xyz/version` with `cat VERSION`.
  A release that fails half way is followed by a new release, never by a re-run of the old one.
- **CloudTrail** shows `AssumeRoleWithWebIdentity` for both sessions, and nothing new under the user:
  `aws iam get-access-key-last-used --access-key-id <ACCESS_KEY_ID>` stays at its pre-merge date.
- **Rollback** while the key still exists: a pull request that reverts the `ci.yaml` change brings the
  access-key path back. Once the key is inactive, reactivate it first (`--status Active`); once it is
  deleted, a rollback is a new key created and stored by a human.
- **Retiring the key** (step 4), in two moves:
  1. On the first green push and release, **deactivate** it -- reversible, and an inactive key is
     useless to whoever can read the secrets:
     `aws iam update-access-key --user-name github-actions-facilitator-deploy --access-key-id <ACCESS_KEY_ID> --status Inactive`.
  2. After one more green release on the roles, **delete** it and the two secrets
     (`aws iam delete-access-key ...`, `gh secret delete AWS_ACCESS_KEY_ID -R UltravioletaDAO/x402-rs`,
     the same for `AWS_SECRET_ACCESS_KEY`), then the user: detach every managed policy
     (`aws iam list-attached-user-policies`), delete every inline one (`aws iam list-user-policies`),
     `aws iam delete-user`. The managed policy `facilitator-cicd-infra` stays: the deploy role holds it.

What the plan role can still read that is not this stack's: `ecs:DescribeTaskDefinition`, which IAM
authorizes on `*` only, returns the plaintext `environment` of every task definition in the account.
Secrets go in `secrets`/`valueFrom`, never in `environment`. A `workflow_dispatch` of `ci.yaml` from a
branch other than `main` now fails at the plan's AssumeRole (the role trusts pull requests and `main`).

The sections below describe the **legacy** access-key user, kept until step 4 removes it.

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
> Its statements, as declared: `DynamoTableManage`, `IamRolePolicyReadForTaskRole`,
> `IamRolePolicyForTaskRoleOnly` (the task role's inline policies, only while the role carries its
> permissions boundary `facilitator-production-ecs-task-boundary`, declared in `main.tf`),
> `DenyPrivilegeEscalation` (Deny), `DenyBoundaryChanges` (Deny: no boundary set or removed, no
> managed policy's versions changed, nothing detached), `BalancesLambdaDeploy`, `AlertTopicManage`,
> `FacilitatorAlarmManage`, `FacilitatorMetricFilterManage`, `FacilitatorScheduleManage`,
> `AlertsBackupQueueManage`, `FacilitatorLogRetention`, `DiscoveryBucketVersioning`,
> `EcsClusterUpdate`, `EcsDeployUntag`, `DenyBazaarTokenDecrypt` (Deny). The `.tf` file is the list;
> this one is a reading aid. `DenyBazaarTokenDecrypt` denies `kms:Decrypt` on the KMS key of the SSM
> SecureString `/facilitator/bazaar-admin-token` (owner's decision 230): `ReadOnlyAccess` grants
> `ssm:GetParameter`, which on that key returns only ciphertext, and the deny keeps it so whatever is
> attached later -- the role `DenySecretValueReads` below plays for Secrets Manager.
> `EcsDeployUntag` (`ecs:UntagResource` on the cluster, the service and the task-definition family)
> is the inverse of the inline policy's `ecs:TagResource`, which it never had: dropping a tag would
> have failed a deploy the same way `ecs:UpdateCluster` did. It covers the ECS half only: dropping a
> key of the provider's `default_tags` also untags the target groups, the security group, the
> subnets, the task roles and the ALB in the deploy's reach, and CI holds none of those untag calls
> — apply such a change by hand. The drift gate's IAM step evaluates both ECS statements against the
> live policy (`scripts/drift_gate_iam.py`).
>
> **Five of those were added only after a deploy failed on their absence** — the pattern this
> document exists to stop:
>
> | When | Missing permission | What it broke |
> |---|---|---|
> | 2026-08-20 | `lambda:UpdateFunctionConfiguration` | The balances Lambda had been silently un-deployable for as long as it existed |
> | 2026-08-28 | `sqs:CreateQueue` | The alerts backup queue |
> | 2026-08-28 | `logs:PutRetentionPolicy` | A log-retention change broke the **routine** deploy for the whole project |
> | 2026-09-10 | `logs:PutMetricFilter` | Every log-based alarm: the deploy could create the alarm but not the metric filter it reads (`FacilitatorMetricFilterManage`) |
> | 2026-10-07 | `ecs:UpdateCluster` | The image deploy of #115: `aws_ecs_cluster.main` is in no target list, but it rides into every image deploy as a dependency of the service, and #115 turned Container Insights off. Granted in `EcsClusterUpdate`, scoped to the one cluster |
>
> **AWS caps a managed policy at 5 versions.** `v1` was deleted by hand on 2026-08-28 to make room
> for `v5`. An apply does **not** fail with `LimitExceeded`: `hashicorp/aws` 5.100.0 deletes the
> oldest non-default version itself before creating the new one, and the plan does not show it.
> Save that version first (`aws iam get-policy-version`) if it matters as a rollback copy.

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

That's it. The next manual "Run workflow" on `main` will build → push to ECR → `terraform apply` → verify.

---

## How it works

| Trigger | `test` job | `deploy` job |
|---|---|---|
| PR or push touching only `docs/**`, `*.md`, `scripts/bench/**` … | **no run at all** (`paths`) | — |
| Pull request → `main` (build input changed) | ✅ build + full test suite | skipped |
| Push → `main` (build input changed, no AWS secrets) | ✅ | **skipped** (run stays green) |
| Push → `main` (build input changed, secrets set) | ✅ | **skipped** — a merge never deploys (since X402-CI-GATES) |
| `workflow_dispatch` on `main` (secrets set) | ✅ | ✅ build → ECR → terraform apply → verify — no `paths`, ships whatever `main` holds |

A superseded pull-request run is cancelled (`cancel-in-progress` is on for `pull_request` and
stays off for `main`, so a running release is never cut between `terraform apply` and the rollout
wait). The concurrency group carries the event: GitHub keeps one pending run per group and cancels
it when a newer one arrives, and with pushes and manual runs in one group a merge could cancel a
release that was waiting in the queue. **Never re-run an old release run**: a re-run joins the group
as the newest and cancels a newer release waiting there. Its own old commit no longer ships — the
deploy job's first step refuses any commit that is not `main`'s head, which used to make it a
silent rollback — so nothing deploys. Start a new one with `gh workflow run ci.yaml --ref main`
instead.

A release now costs two runs: the merge's push run (test + drift gate) and the manual run (test +
drift gate + deploy). The second test is on purpose — `deploy` needs `test`, and the manual run
ships whatever `main` holds when it starts — but it is ~20-35 more minutes of Actions per release.

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
- **Balances Lambda:** a **separate step**, applied only when `lambda/balances/**`,
  `lambda-balances.tf` or the other files its step lists changed **since the last deploy that
  went green** — not since the push's parent, which lost every change whose own deploy failed
  before reaching the step. `scripts/ci_last_green_deploy.py` picks that commit (the head SHA of
  the newest run of this workflow on `main` whose deploy job succeeded, read from the Actions runs
  and jobs APIs) and the compare API lists the files. Until 2026-08-20 the
  Lambda was excluded from every run, and the zip-hash reason above was only half the story — the
  deploy user held **no `lambda:*` write permissions at all**, so a full apply could not have
  succeeded either. A change to `lambda/balances/` would land in `main` and never reach AWS: that is
  how `RPC_URL_SUI` stayed pointed at a dead endpoint in the Lambda while the same commit fixed it
  for the facilitator. The `BalancesLambdaDeploy` statement in `facilitator-cicd-infra` (added
  2026-08-20, scoped to that one function ARN) grants the four writes Terraform needs.
  If that commit cannot be determined, or the compare fails or reaches its 300-file cap, the step
  applies anyway — a redundant `UpdateFunctionCode` is cheap, a silently unapplied change is the
  bug being fixed. A `workflow_dispatch` run always applies the Lambda: that was the manual escape
  hatch to resync it whenever it drifted from `main`, and since every deploy is a
  `workflow_dispatch` run (X402-CI-GATES) the Lambda applies on every release. The observability step
  below applies it as well whenever it has a pending diff: two of its targets, the schedule's event
  target and its Lambda permission, reference the function (`alerts.tf:346-355`), and `-target`
  pulls in their dependencies.
- **Observability and the discovery bucket:** two more targeted applies (alarms, SNS, SQS, the
  balances schedule and metric filters; the discovery bucket's versioning and lifecycle), on
  **every** deploy. They used to apply only when the push's own diff touched their files, so a
  change that arrived with a failed deploy was never applied by the pushes after it (the
  `alerts-solana-mint.tf` alarms, 2026-09-10). With nothing to change, each is a no-op of seconds.
- **Verify:** waits for `services-stable`, then polls `/health` for `200`. Both run whenever the
  image apply ran, **even if it or a step after it failed** — a failed observability step used to
  skip them and leave the new image live with no verdict (2.19.0), and so did an image apply that
  failed after moving the service (2026-09-26). They skip only when the apply never ran. The job
  still ends red.
- `tests/scripts/test_ci_*.py` (run by the `test` job) hold that shape: the wait and the health
  check key on the image apply having run, and no deploy step skips on `github.event.before`.
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

1. ~~Migrate to GitHub OIDC~~ — done in X402-SEG-CI (`cicd-oidc.tf`; the account's provider for
   `token.actions.githubusercontent.com` already existed and is referenced, not declared). Pending
   only the switch-on steps at the top of this file.
2. **Require a manual gate for prod** — the `deploy` job now runs in the **`production`**
   environment; add required reviewers to it if you want a human approval before each apply.
3. ~~Scope `iam:PassRole`~~ — done: the deploy role passes only the execution, task and balances
   Lambda roles, each with `iam:PassedToService` (`cicd-oidc.tf`, `PassRoleTo*`).
4. **Rotate** the legacy access key until step 4 of the switch-on deletes it.
