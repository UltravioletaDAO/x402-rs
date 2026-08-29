# ============================================================================
# Variable values for the production environment -- VERSIONED ON PURPOSE.
#
# Terraform auto-loads any *.auto.tfvars in the working directory, so this file
# applies identically to a local `terraform apply` and to CI. That is the whole
# point of it.
#
# WHY THIS FILE EXISTS
#
# `terraform.tfvars` is gitignored. CI checks the repo out without it, so every
# apply in the pipeline ran on the DEFAULTS in variables.tf while operators ran
# on their local tfvars. Two sources of truth for the same value, one of them
# invisible to code review. When they disagreed the deploy silently won:
# CloudTrail on 2026-08-14 caught the CI role putting alb_idle_timeout back from
# 600 to 180, thirteen minutes after an operator set it, deploy after deploy.
#
# The defence up to now was a comment on each variable telling the next person to
# remember to edit two files. It is the same shape of defence that failed for
# image_tag in image-pin.tf, and for the same reason: it competes with attention,
# and attention goes to whatever the operator came to change.
#
# So the value lives here instead, in the repo, where CI and the operator read
# the SAME bytes and every change to one is a reviewable diff.
#
# NOTHING SECRET GOES IN THIS FILE. These are operational knobs -- capacity,
# timeouts, retention, feature flags -- plus the NAMES (never the contents) of
# Secrets Manager entries. Credentials are fetched at runtime from Secrets
# Manager; see secrets.tf. Every value below was already published verbatim as a
# default in variables.tf, so committing this file discloses nothing new. If a
# value would be damaging to publish, it does not belong here: put it in Secrets
# Manager and reference it.
#
# PRECEDENCE, AND THE ONE THING THAT WILL SURPRISE YOU
#
# *.auto.tfvars loads AFTER terraform.tfvars and beats it. Measured, not assumed:
# with 512 in terraform.tfvars and 1024 here, `terraform console` answers 1024.
#
# So a leftover terraform.tfvars is now WORSE than useless -- it reads like an
# override and is silently inert. Delete it. To override deliberately for one
# local run, put the value in a file of any other name and pass it on the command
# line, which does win:
#
#   terraform plan -var-file=local.tfvars        # or: -var 'desired_count=1'
#
# The defaults in variables.tf likewise stop being the operative value. They
# remain documentation and a safety net, and keeping them in agreement is still
# polite, but they are no longer the only thing standing between an operator and
# a silent revert.
#
# image_tag is deliberately ABSENT. Read its comment in variables.tf: a value
# parked in a tfvars file ages into a rollback instruction, which is how
# production went back two months on 2026-08-03. CI passes it with -var on every
# release; a bare apply falls through to the image already running.
# ============================================================================

# General
aws_region  = "us-east-2"
environment = "production"

# Network
vpc_cidr           = "10.1.0.0/16"
availability_zones = ["us-east-2a", "us-east-2b"]
single_nat_gateway = true

# ECS task sizing
task_cpu    = 1024 # 1 vCPU
task_memory = 2048 # 2 GB

# desired_count carries `ignore_changes` on aws_ecs_service.facilitator (main.tf),
# so Terraform never writes it and this value cannot move the running count.
# min_capacity is what actually holds the floor.
desired_count = 2
min_capacity  = 2 # a single task is not a service
max_capacity  = 3

# Auto-scaling targets
alb_request_count_target_value = 15 # req/min/target -- the primary signal; CPU never fires here
memory_target_value            = 80

# ALB
alb_idle_timeout = 600 # must outlast the facilitator's own 300s timeout

# DNS
domain_name      = "facilitator.ultravioletadao.xyz"
hosted_zone_name = "ultravioletadao.xyz"

# Secrets Manager entry NAMES (identifiers, never contents)
evm_secret_name       = "facilitator-evm-private-key"
solana_secret_name    = "facilitator-solana-keypair"
quicknode_secret_name = "facilitator-quicknode-base-rpc"

# CloudWatch
log_retention_days        = 30   # 7 lost the 2026-08-10 incident to expiry
enable_container_insights = true # this IS what the cluster runs -- verified with --include SETTINGS

# Container registry
ecr_repository_name = "facilitator"

# Observability stack (Grafana + Prometheus + Tempo) -- off is $0/month
enable_observability = false
