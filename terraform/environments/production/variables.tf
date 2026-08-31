# Terraform Variables for Facilitator Production Environment

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-2"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "production"
}

variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.1.0.0/16"
}

variable "availability_zones" {
  description = "Availability zones"
  type        = list(string)
  default     = ["us-east-2a", "us-east-2b"]
}

# use_fargate_spot, use_nat_instance and enable_vpc_endpoints used to live here.
# Deleted 2026-08-29: none of the three was read by any resource (repo-wide grep, drift
# audit) -- the facilitator always runs FARGATE (hardcoded, not var-driven), egress is
# always an aws_nat_gateway regardless of use_nat_instance's value, and the DynamoDB/S3/
# Secrets Manager VPC endpoints in main.tf are unconditional, not gated by
# enable_vpc_endpoints. All three tfvars comments promised savings that were not real:
# DynamoDB and S3 endpoints are Gateway-type and already free ($0, confirmed via the AWS
# Price List API); only the Secrets Manager Interface endpoint has a real cost, and that is
# ~$7.30/mo for the single AZ it runs in today ($0.01/hr, us-east-2, single_nat_gateway
# mode pins it to one ENI) -- not the "$35/month" the dead enable_vpc_endpoints comment
# claimed. Disabling it would also reopen exactly the egress surface the B10 hardening in
# main.tf (VPC endpoints for AWS-service traffic instead of the public NAT edge) was built
# to close, for a saving under $10/mo. Not worth wiring up.
#
# docs/COST_RIGHTSIZING_HANDOFF_2026-08-07.md already caught this same discrepancy (tfvars
# claiming a NAT *instance* while a NAT *Gateway* was actually running) and explicitly
# flagged it as something to reconcile before trusting any `terraform plan` here. This
# deletion is that reconciliation. See that handoff for the NAT Gateway -> instance swap
# itself (~$25/mo, deliberately out of scope -- a money-path network change needs its own
# window, not a drive-by alongside a dead-variable cleanup).

variable "single_nat_gateway" {
  description = "Use single NAT gateway (true) or one per AZ (false)"
  type        = bool
  default     = true
}

variable "task_cpu" {
  description = <<-EOT
    Fargate task CPU units (1024 = 1 vCPU).

    docs/COST_RIGHTSIZING_HANDOFF_2026-08-07.md recommends 1024 -> 512 (~$14.8/mo) off
    measured July usage (avg CPU 1.8%). DO NOT apply that on its own -- it is gated on an
    open question from docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md
    ("Lo que quedo sin verificar"): `#[tokio::main]` does not pin worker_threads, so tokio
    sizes the runtime from available_parallelism(), which on Linux reads
    sched_getaffinity() (the CPU affinity mask) -- NOT the cgroup quota. Nobody has
    confirmed whether 1024 units resolves to 1 worker or N. Halving to 512 pushes that
    number toward 1, and a single-worker tokio runtime means any CPU-bound stretch without
    an .await freezes the whole server, not just one request.

    2026-08-29: closed the measurement gap, not the question. src/main.rs now logs
    `workers=N tokio worker threads` at boot (info!, next to the other effective-config
    lines). The next deploy answers this with `aws logs filter-log-events` -- no ECS Exec,
    no dedicated investigation -- read that number before touching this value. 1 worker
    keeps 1024; N workers with headroom makes 512 defensible.
  EOT
  type    = number
  default = 1024
}

variable "task_memory" {
  description = "Fargate task memory in MB"
  type        = number
  default     = 2048
}

variable "desired_count" {
  description = "Desired number of tasks. Default 2 spreads tasks across AZs so a single-AZ outage does not drop the service. Override to 1 only for dev/cost-saving (acknowledge the resilience tradeoff)."
  type        = number
  default     = 2
}

variable "min_capacity" {
  description = <<-EOT
    Minimum number of tasks for auto-scaling.

    Was 1 until 2026-08. That meant the service ran a single task with no
    floor -- autoscaling only ever added capacity on top of one, never
    protected against that one task being the entire service. 2 gives a
    resilience floor independent of any scaling policy: Application Auto
    Scaling enforces min_capacity as a hard bound and will scale OUT to reach
    it even when no policy's metric calls for more capacity, so raising this
    alone is what actually moves desired_count off of 1 (desired_count itself
    has `ignore_changes` on aws_ecs_service.facilitator -- see main.tf).

    terraform.tfvars is gitignored (CI runs on this default) -- keep the
    local tfvars value in agreement or you reproduce the alb_idle_timeout bug.
  EOT
  type        = number
  default     = 2
}

variable "max_capacity" {
  description = "Maximum number of tasks for auto-scaling"
  type        = number
  default     = 3
}

variable "memory_target_value" {
  description = "Target memory utilization for auto-scaling"
  type        = number
  default     = 80
}

variable "alb_request_count_target_value" {
  description = <<-EOT
    Target ALBRequestCountPerTarget for auto-scaling -- requests per minute,
    per running task. This is the PRIMARY scaling signal, replacing CPU.

    Why not CPU: measured across three degradation episodes (2026-08 diagnosis,
    docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md), CPU never
    exceeded 25% while the service was visibly struggling -- the facilitator is
    I/O-bound (waiting on RPC calls), so a CPU-target policy at 75% never fires
    and is decorative for this workload. Request count actually tracks load.

    Calibration (measured, 2026-08): baseline 900-1200 req/h, incidents
    2600-3300 req/h. Those numbers were measured with ONE task running, so at
    min_capacity=2 they roughly halve per target: baseline ~7.5-10 req/min/
    target, incident onset ~22-27.5 req/min/target. 15 sits with headroom above
    the baseline ceiling but well below where incidents start, so scale-out
    fires proactively instead of after the fact. Re-measure and adjust once
    real 2-task traffic data exists -- this number is a first calibration, not
    a permanent constant.
  EOT
  type        = number
  default     = 15
}

variable "alb_idle_timeout" {
  description = "ALB idle timeout in seconds"
  type        = number
  # 600, matching terraform.tfvars, and the two MUST stay in agreement.
  #
  # terraform.tfvars is gitignored, so CI checks out this repo without it and
  # every `terraform apply` there runs on these defaults. The deploy's targeted
  # apply pulls `aws_lb.main` in as a dependency of the ECS service, so a default
  # that disagreed with the tfvars value did not sit there harmlessly: it
  # REVERTED the setting on every single deploy. CloudTrail on 2026-08-14 shows
  # exactly that -- an operator setting 600, and the CI role putting it back to
  # 180 thirteen minutes later, deploy after deploy.
  #
  # The value itself is load-bearing: an Ethereum L1 escrow settle can run past
  # 60s and the facilitator's own timeout is 300s, so the ALB has to outlast it
  # or the client gets a 504 for a payment that is still in flight.
  default = 600
}

variable "domain_name" {
  description = "Domain name for facilitator"
  type        = string
  default     = "facilitator.ultravioletadao.xyz"
}

variable "hosted_zone_name" {
  description = "Route53 hosted zone name"
  type        = string
  default     = "ultravioletadao.xyz"
}

variable "evm_secret_name" {
  description = "AWS Secrets Manager secret name for EVM private key"
  type        = string
  default     = "facilitator-evm-private-key"
}

variable "solana_secret_name" {
  description = "AWS Secrets Manager secret name for Solana keypair"
  type        = string
  default     = "facilitator-solana-keypair"
}

variable "quicknode_secret_name" {
  description = "AWS Secrets Manager secret name for QuickNode RPC (optional)"
  type        = string
  default     = "facilitator-quicknode-base-rpc"
}

variable "log_retention_days" {
  description = <<-EOT
    CloudWatch log retention in days for /ecs/facilitator-<environment>.

    Was 7. The 2026-08-10 degradation episode aged out of the log group before
    anyone got to it, losing the only application-level record of that
    incident. 30 keeps roughly a month of history without the unbounded-cost
    risk of "forever" -- this is the app log group only, not an audit trail.

    terraform.tfvars is gitignored (CI runs on this default) -- keep the
    local tfvars value in agreement.
  EOT
  type        = number
  default     = 30
}

variable "enable_container_insights" {
  description = <<-EOT
    Enable ECS Container Insights (per-container CPU/memory/network metrics beyond the
    standard ECS service-level metrics, which are unaffected either way).

    Stays true -- this IS what AWS is actually running. A 2026-08-29 drift-audit entry
    briefly flipped this default to false on the belief that AWS had it disabled; that belief
    was wrong. `aws ecs describe-clusters` omits the `settings` field entirely unless you
    pass `--include SETTINGS` -- without that flag it silently returns `[]`, which reads
    exactly like "disabled" if you do not know the flag exists. Two independent reads with
    `--include SETTINGS` (plan-gate, and re-verified here) both show
    `{"name": "containerInsights", "value": "enabled"}`. Left as a warning for the next
    person auditing this cluster with the CLI: always pass --include SETTINGS, or you will
    read the omission as the value.

    docs/COST_RIGHTSIZING_HANDOFF_2026-08-07.md still recommends turning this off
    deliberately (~$10-15/mo) -- that recommendation was never wrong, only the claim that it
    had already happened.
  EOT
  type    = bool
  default = true
}

variable "ecr_repository_name" {
  description = "ECR repository name"
  type        = string
  default     = "facilitator"
}

variable "image_tag" {
  description = <<-EOT
    Docker image tag to deploy. Pass it on the command line (`-var image_tag=…`),
    which is what CI does on every release.

    Leave it UNSET in terraform.tfvars. A value parked in that file ages on its
    own — CI deploys by applying with the flag and never writes back — and then
    every unrelated apply carries it as a silent rollback instruction. That is
    not hypothetical: on 2026-08-03 a stale 1.47.0 in tfvars took production
    back two months during an apply meant to add one environment variable.

    Unset, a bare apply redeploys whatever is already running (see image-pin.tf)
    and cannot change the version by accident.
  EOT
  type        = string
  default     = ""
}

variable "enable_dx402" {
  description = <<-EOT
    Turn on the DX402 `durable-evidence` extension.

    The bucket, table and IAM policies in dx402.tf are created regardless — they
    cost effectively nothing idle — so provisioning and switching on are separate
    steps. This flag only controls the environment the container receives.

    Prerequisite: the secret `facilitator-dx402-signing-key` must EXIST before
    setting this to true, or the apply fails resolving the data source. Create it
    with scripts/dx402-bootstrap-secret.sh.

    Off, the facilitator never advertises `durable-evidence` in /supported and
    the /dx402/* routes are not registered at all, so nothing on the payment path
    changes.

    DEFAULT IS TRUE, and it has to be. terraform.tfvars is gitignored, so CI
    applies with the defaults in this file. A default that disagrees with the
    local tfvars silently reverts the setting on every deploy -- that is exactly
    how alb_idle_timeout sat reverted for months. If you turn DX402 off, turn it
    off HERE.
  EOT
  type        = bool
  default     = true
}

variable "dx402_storage_backend" {
  description = <<-EOT
    Where DX402 anchors sealed evidence: "s3" (default) or "ipfs" (Pinata).

    With "ipfs", Pinata sits IN FRONT of S3 rather than replacing it: an outage
    costs latency, never the evidence, and the record says where the bytes
    actually landed. The fallback only ever goes toward the more conservative
    store -- S3 is private, deletable, and its retention is enforced by a bucket
    rule -- so it can never turn a revocable promise into an irrevocable one.

    Requires the `facilitator-dx402-pinata` secret to exist, and the retention
    sweeper (spawn_retention_sweeper) to be running -- Pinata expires nothing on
    its own, so without it `retentionUntil` would be a promise with no mechanism
    while every receipt carries our signature over it. That is why this stayed
    "s3" until the sweeper existed.
  EOT
  type        = string
  default     = "ipfs"

  validation {
    condition     = contains(["s3", "ipfs"], var.dx402_storage_backend)
    error_message = "dx402_storage_backend must be \"s3\" or \"ipfs\"."
  }
}

variable "dx402_allow_public_ipfs" {
  description = <<-EOT
    Whether to OFFER the `ipfs-public` backend at all.

    Off on purpose, and not because of a missing credential. Public IPFS is
    irreversible: unpinning removes our copy, not the network's, so the
    `retentionUntil` the facilitator SIGNS stops being true. And the ciphertext
    that becomes permanent is the BUYER's, who today has no way to consent --
    that arrives with the `accepts` opt-in.

    Turning this on is a decision about somebody else's data. Read
    docs/plans/dx402/06-PLAN-PINATA.md before you do.
  EOT
  type        = bool
  default     = false
}

variable "dx402_pinata_gateway" {
  description = <<-EOT
    The Pinata account's own gateway domain, e.g.
    "amaranth-broad-whippet-395.mypinata.cloud".

    Required for the ipfs backend: minting a signed URL for a private object
    against the generic `gateway.pinata.cloud` answers 403. Not a secret -- it is
    a hostname, and a read still needs a URL only we can sign.
  EOT
  type        = string
  default     = "amaranth-broad-whippet-395.mypinata.cloud"
}

variable "alerts_email" {
  description = <<-EOT
    Email subscribed to the facilitator's own SNS alert topic. Empty disables the
    subscription (the topic and alarms are still created, they just reach nobody).
    AWS sends a one-time confirmation link that a human must click; until then the
    subscription reads "PendingConfirmation" and delivers nothing.

    NOTE: terraform.tfvars is gitignored, so CI runs on this default. Unlike a
    capacity setting, a wrong value here fails quietly -- the alarms work and the
    mail goes nowhere. Keep the default correct.
  EOT
  type        = string
  default     = "0xultravioleta@gmail.com"
}

variable "alb_access_logs_enabled" {
  description = <<-EOT
    Turn on ALB access logging to S3 (see alb-access-logs.tf).

    DEFAULT IS FALSE ON PURPOSE, and flipping it is a TWO-STEP rollout, not a
    flag flip:

    1. Apply with this still false. That creates the bucket, its policy and
       lifecycle rule WITHOUT touching aws_lb.main -- the bucket name is a
       plain string local, not a resource reference, so aws_lb.main carries no
       dependency edge to it and CI's routine targeted deploy
       (-target=aws_ecs_task_definition.facilitator -target=aws_ecs_service.facilitator,
       which pulls in aws_lb.main via the listener depends_on chain) stays
       completely unaffected by this file.
    2. ONLY once the bucket exists, flip this to true and apply again. That is
       the change that touches aws_lb.main (adds the access_logs block) --
       and BECAUSE aws_lb.main is already inside CI's targeted graph, that
       diff ships on the very next routine deploy after you merge it. If the
       bucket does not exist yet when that happens, the ALB update fails and
       breaks that deploy for everyone -- exactly the failure mode
       docs/handoffs/2026-08-22-continuar-desde-wsl-alertas-y-performance.md
       hit with the balances Lambda. Do not merge step 2 before step 1 has
       actually been applied and verified.

    terraform.tfvars is gitignored (CI runs on this default) -- if you flip
    this in tfvars for a local apply, flip it here too before merging, or the
    next CI deploy reverts it.
  EOT
  type        = bool
  default     = false
}

variable "alb_access_logs_retention_days" {
  description = <<-EOT
    How long ALB access logs live in S3 before lifecycle expiration.

    These exist to make HTTP-level failures (e.g. the 460s that were 201 of
    417 real failures in the 2026-08 diagnosis) visible after the fact, not to
    be a permanent record -- 90 days covers "we noticed weeks later" without
    unbounded storage growth on a bucket that gets one object per request.
  EOT
  type        = number
  default     = 90
}

variable "fhe_request_timeout_secs" {
  description = <<-EOT
    How long the facilitator waits for the Zama FHE Lambda before giving up.

    Reaches the container as FHE_PROXY_TIMEOUT_SECS and drives the reqwest
    client in src/fhe_proxy.rs. The Rust default matches this number so an
    unset environment behaves identically; nothing else in the code retypes it.

    MIRROR: this must equal `fhe_request_timeout_secs` in
    terraform/environments/zama-testnet/variables.tf, which is the canonical
    definition for the Lambda side (function timeout, API Gateway integration,
    duration alarm). They are two separate Terraform states, so a variable
    cannot be shared -- change one, change the other. `terraform output
    fhe_request_timeout` in the zama-testnet workspace prints the effective
    value at every hop.

    Note the ceiling that is not ours: the Lambda sits behind an API Gateway
    HTTP API, whose 30s integration timeout AWS marks as not increasable. Until
    that entry point changes, a caller is cut off at 30s no matter what this
    says. 90 is the value the stack agrees on, not the wait a client gets.
  EOT
  type        = number
  default     = 90

  validation {
    condition     = var.fhe_request_timeout_secs >= 3 && var.fhe_request_timeout_secs <= 900
    error_message = "FHE request timeout must be between 3 and 900 seconds (matches src/fhe_proxy.rs)."
  }
}
