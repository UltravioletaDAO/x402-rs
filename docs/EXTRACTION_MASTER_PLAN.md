# Facilitator Extraction Master Plan

> **Mission**: Extract x402-rs facilitator from karmacadabra into a standalone, self-contained repository at `z:\ultravioleta\dao\karmacadabra\facilitator\` ready to become its own Git repository.

**Document Version**: 1.0
**Created**: 2025-11-01
**Status**: Draft - Awaiting User Decisions

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Critical Context & History](#critical-context--history)
3. [File Inventory](#file-inventory)
4. [Infrastructure Analysis](#infrastructure-analysis)
5. [Extraction Phases](#extraction-phases)
6. [Key Decision Points](#key-decision-points)
7. [Risk Assessment](#risk-assessment)
8. [Success Criteria](#success-criteria)

---

## Executive Summary

### What We're Extracting

The **x402-rs facilitator** is production payment infrastructure serving 17 blockchain networks:
- **Mainnets (7)**: Base, Avalanche, Celo, HyperEVM, Polygon, Solana, Optimism
- **Testnets (10)**: Base Sepolia, Avalanche Fuji, Celo Sepolia, HyperEVM Testnet, Polygon Amoy, Solana Devnet, Optimism Sepolia, Sei, Sei Testnet, XDC

**Current State**: Embedded in karmacadabra monorepo with:
- Custom Ultravioleta DAO branding (57KB landing page + assets)
- Shared AWS infrastructure (ECS cluster, VPC, ALB)
- Dependencies scattered across scripts/, tests/, terraform/, docs/

**Target State**: Fully self-contained `facilitator/` directory that can:
- Run independently with `docker-compose up`
- Deploy to AWS with standalone Terraform
- Preserve all custom branding and network configurations
- Maintain backward compatibility with karmacadabra agents

### Why Extract Now

1. **Architectural Independence**: Facilitator is Layer 2, agents are Layer 3 - clean separation
2. **Reusability**: Other projects can use facilitator without pulling karmacadabra
3. **Deployment Isolation**: Can deploy/update facilitator without affecting agents
4. **Maintenance Clarity**: Upstream x402-rs updates shouldn't touch karmacadabra
5. **Security Scope**: Separate repos = separate security audits

### Timeline & Effort

- **Discovery & Planning**: 6-8 hours (in progress)
- **Extraction Execution**: 6-8 hours
- **Testing & Validation**: 4-6 hours
- **Infrastructure Migration**: 8-12 hours (includes AWS deployment)
- **Documentation**: 3-4 hours
- **Production Cutover**: 2-3 hours + monitoring
- **Total**: 29-41 hours over 3-4 weeks

### Cost Impact

**Current**: $46-56/month (shared infrastructure)
**Standalone (initial)**: $113-123/month (+$76/month)
**Standalone (optimized)**: $41-51/month (+$4-10/month) ✅ **Acceptable**

Optimizations: NAT instance vs NAT Gateway, right-sized tasks (1 vCPU/2GB), remove VPC endpoints

---

## Critical Context & History

### Previous Incident: Upstream Merge Overwrote Branding

**Date**: During 0.7.9 → 0.9.0 upgrade
**What Happened**: Used `cp -r upstream/* x402-rs/` which **OVERWROTE**:
- 57KB Ultravioleta DAO landing page (replaced with "Hello from x402-rs!" text)
- Custom `get_root()` handler using `include_str!("../static/index.html")`
- All static assets (logos, favicon, network images)

**Impact**: Live public-facing endpoint showed broken branding on live streams

**Recovery**: Git history restoration + handler rewrite + Docker rebuild + ECS redeploy

**Lesson**: **NEVER use mass file copy from upstream. ALWAYS use git merge strategy.**

### Protected Files - NEVER Overwrite

**Tier 1: CRITICAL - Immediate Production Breakage**
```
x402-rs/static/                      # ENTIRE FOLDER
├── index.html                       # 57,662 bytes - Ultravioleta DAO branding
├── favicon.ico                      # DAO favicon
├── logo.png                         # DAO logo
└── images/                          # Network logos
    ├── avalanche.png
    ├── base.png
    ├── celo.png
    ├── hyperevm.png
    ├── polygon.png
    ├── solana.png
    └── optimism.png

x402-rs/Dockerfile                   # Custom: rustup default nightly
```

**Tier 2: CRITICAL - Silent Integration Failures**
```
x402-rs/src/handlers.rs              # Lines ~76-85: include_str!("../static/index.html")
x402-rs/src/network.rs               # ALL 17 FUNDED NETWORKS - NEVER REMOVE
```

### Verification Checklist (Must Pass After Extraction)

- [ ] Landing page shows "Ultravioleta DAO" branding
- [ ] All 12 network logos display correctly (avalanche.png, base.png, etc.)
- [ ] `curl http://localhost:8080/` returns 57KB+ HTML (not text)
- [ ] `grep -q "Ultravioleta DAO" facilitator/static/index.html`
- [ ] `grep -q "include_str!" facilitator/src/handlers.rs`
- [ ] All 17 networks in `facilitator/src/network.rs`

---

## File Inventory

### Core Facilitator Files (100% Move)

**x402-rs/ Directory** (entire directory moves):
```
x402-rs/
├── src/                             # Rust source (11 files)
│   ├── main.rs
│   ├── handlers.rs                  # CRITICAL: Custom get_root() handler
│   ├── network.rs                   # CRITICAL: 17 custom networks
│   ├── facilitator.rs
│   ├── facilitator_local.rs
│   ├── lib.rs
│   ├── provider_cache.rs
│   ├── telemetry.rs
│   ├── timestamp.rs
│   ├── types.rs
│   └── chain/                       # Blockchain integrations
│       ├── mod.rs
│       ├── evm.rs
│       └── solana.rs
├── static/                          # CRITICAL: Ultravioleta DAO branding
│   ├── index.html                   # 57,662 bytes
│   ├── favicon.ico
│   ├── logo.png
│   ├── avalanche.png
│   ├── base.png
│   ├── celo.png
│   ├── celo-colombia.png
│   ├── hyperevm.png
│   ├── optimism.png
│   ├── polygon.png
│   ├── solana.png
│   ├── README.md
│   └── SETUP.md
├── crates/                          # Workspace crates
│   ├── x402-axum/                   # Server middleware
│   │   ├── src/
│   │   ├── Cargo.toml
│   │   └── README.md
│   └── x402-reqwest/                # Client middleware
│       ├── src/
│       ├── Cargo.toml
│       └── README.md
├── examples/                        # Usage examples
│   ├── x402-axum-example/
│   └── x402-reqwest-example/
├── .cargo/
│   └── config.toml
├── Cargo.toml                       # CRITICAL: Workspace config
├── Dockerfile                       # CRITICAL: Custom nightly Rust
├── README.md
├── CHANGELOG.md
├── CUSTOMIZATIONS.md
├── DEPLOYMENT.md
├── EIP3009_TIMESTAMP_BEST_PRACTICES.md
└── LANDING_PAGE.md
```

**Files to Move**: ~80 files
**Total Size**: ~15 MB (excluding target/)
**Git History**: Preserve commits touching x402-rs/

### Testing Files (Move to facilitator/tests/)

**Python Integration Tests** (28 files in scripts/):
```
scripts/test_glue_payment_simple.py          → facilitator/tests/test_glue_payment.py
scripts/test_usdc_payment_base.py            → facilitator/tests/test_usdc_payment.py
scripts/test_base_usdc_stress.py             → facilitator/tests/test_payment_stress.py
scripts/test_facilitator_verbose.py          → facilitator/tests/test_facilitator.py
scripts/test_real_x402_payment.py            → facilitator/tests/test_x402_integration.py
scripts/test_glue_quick.py                   → facilitator/tests/test_quick_payment.py
scripts/test_complete_flow.py                → facilitator/tests/test_complete_flow.py
scripts/test_all_endpoints.py                → facilitator/tests/test_endpoints.py

# Diagnostic scripts
scripts/check_facilitator_config.py          → facilitator/scripts/check_config.py
scripts/check_facilitator_version.py         → facilitator/scripts/check_version.py
scripts/diagnose_usdc_payment.py             → facilitator/scripts/diagnose_payment.py
scripts/compare_domain_separator.py          → facilitator/scripts/compare_domain_separator.py
scripts/compare_usdc_contracts.py            → facilitator/scripts/compare_contracts.py
scripts/verify_full_stack.py                 → facilitator/scripts/verify_stack.py

# Test data
scripts/usdc_contracts_facilitator.json      → facilitator/tests/fixtures/usdc_contracts.json
```

**Rust Tests** (tests/ directory):
```
tests/test_facilitator.py                    → facilitator/tests/integration/test_facilitator.py
tests/x402/python/test_facilitator.py        → facilitator/tests/integration/test_x402.py
tests/x402/README.md                         → facilitator/tests/x402/README.md
tests/x402/TROUBLESHOOTING.md                → facilitator/tests/x402/TROUBLESHOOTING.md
tests/x402/payloads/                         → facilitator/tests/x402/payloads/ (all .json files)
tests/x402/load/                             → facilitator/tests/load/ (k6, artillery configs)
```

**Test Seller** (test-seller/):
```
test-seller/test_facilitator_direct.py       → facilitator/tests/integration/test_direct.py
test-seller/FACILITATOR_BUG_REPORT.md        → facilitator/docs/bug-reports/
```

**Files to Move**: ~35 test files
**Action**: Copy (don't move) - karmacadabra may still need some tests

### Infrastructure Files (Complex - See Infrastructure Section)

**Terraform** (terraform/ecs-fargate/):
```
terraform/ecs-fargate/main.tf                # Extract facilitator resources
terraform/ecs-fargate/variables.tf           # Extract facilitator variables
terraform/ecs-fargate/alb.tf                 # Extract facilitator target group + rules
terraform/ecs-fargate/route53.tf             # Extract facilitator DNS
terraform/ecs-fargate/acm.tf                 # Extract facilitator certificate
terraform/ecs-fargate/FACILITATOR_MAINNET_MIGRATION.md
terraform/ecs-fargate/INFRASTRUCTURE_ANALYSIS_REPORT.md
```

**Task Definitions**:
```
facilitator-task-def-mainnet.json            → facilitator/terraform/task-definitions/
facilitator-task-def-mainnet-v2.json         → facilitator/terraform/task-definitions/
```

**Decision**: COPY entire terraform/ and simplify (remove multi-agent loops)
**Rationale**: Standalone infrastructure, no shared state dependencies

### Deployment & Operations Scripts

**AWS Deployment**:
```
scripts/build-and-push.py                    → facilitator/scripts/build-and-push.py (extract facilitator section)
scripts/deploy-to-fargate.py                 → facilitator/scripts/deploy.py (extract facilitator section)
scripts/upgrade_facilitator.ps1              → facilitator/scripts/upgrade.ps1
```

**Secrets Management**:
```
scripts/setup_facilitator_secrets.py         → facilitator/scripts/setup_secrets.py
scripts/migrate_facilitator_secrets.py       → facilitator/scripts/migrate_secrets.py
scripts/split_facilitator_secrets.py         → facilitator/scripts/split_secrets.py
scripts/rotate-facilitator-wallet.py         → facilitator/scripts/rotate_wallet.py
scripts/create_testnet_facilitator_secret.py → facilitator/scripts/create_testnet_secret.py
```

**Files to Move**: ~10 deployment scripts
**Action**: Extract facilitator-specific code, leave originals in karmacadabra

### Documentation Files

**Facilitator-Specific Docs**:
```
docs/FACILITATOR_TESTING.md                  → facilitator/docs/TESTING.md
docs/FACILITATOR_WALLET_ROTATION.md          → facilitator/docs/WALLET_ROTATION.md
docs/X402_FORK_STRATEGY.md                   → facilitator/docs/UPSTREAM_MERGE_STRATEGY.md
docs/migration/FACILITATOR_SECRETS_MIGRATION.md → facilitator/docs/SECRETS_MIGRATION.md
FACILITATOR_VALIDATION_BUG.md                → facilitator/docs/bug-reports/validation-bug.md
```

**Bug Reports & Analysis** (root level):
```
BASE_USDC_BUG_INVESTIGATION_REPORT.md        → facilitator/docs/bug-reports/base-usdc-bug.md
AWS_INFRASTRUCTURE_ANALYSIS_2025-10-31.md    → facilitator/docs/infrastructure-analysis.md
```

**Architecture Docs** (docs/images/architecture/):
- Extract sections mentioning facilitator from architecture mermaid diagrams
- Create new facilitator-specific architecture diagrams

**Files to Move**: ~10 documentation files
**Action**: Copy + adapt (remove karmacadabra-specific references)

### Configuration Files

**Environment Variables**:
```
x402-rs/.env.example                         → facilitator/.env.example
# NOTE: .env is gitignored, NOT moved
```

**Docker**:
```
docker-compose.yml                           → Extract facilitator service definition
docker-compose.dev.yml                       → Extract facilitator service definition
```

**CI/CD** (if exists):
```
.github/workflows/*                          → Extract facilitator-related workflows
```

**AWS Secrets** (to recreate):
```
karmacadabra-facilitator-mainnet             → facilitator-evm-private-key
karmacadabra-solana-keypair                  → facilitator-solana-keypair
karmacadabra-quicknode-base-rpc              → facilitator-quicknode-base-rpc (optional)
```

### Files to KEEP in Karmacadabra (Reference Only)

**Agent Configuration**:
- All agent code references facilitator via URL: `https://facilitator.karmacadabra.ultravioletadao.xyz`
- No code imports from x402-rs/ (only HTTP calls)
- `.env` files have `FACILITATOR_URL` variable

**Shared Libraries**:
- `shared/` directory (no direct dependencies found)

**Root Documentation**:
- `README.md` - Update to reference external facilitator repo
- `CLAUDE.md` - Keep x402-rs upgrade section (historical reference)
- `MASTER_PLAN.md` - Update Layer 2 architecture references

---

## Infrastructure Analysis

### Current AWS Deployment

**ECS Cluster**: `karmacadabra-prod` (SHARED with 7 agents)
- Region: `us-east-1`
- Services: facilitator, validator, karma-hello, abracadabra, skill-extractor, voice-extractor, (2 more)

**Facilitator Service**: `karmacadabra-prod-facilitator`
- Task Definition: 2 vCPU / 4 GB (Fargate on-demand, NOT Spot)
- Desired Count: 1 task
- Auto-scaling: 1-3 tasks (CPU 75%, Memory 80%)
- Health Check: `/health` endpoint (60s grace period)

**Shared Resources** (8 services):
- VPC: `10.0.0.0/16` (2 AZs, public + private subnets)
- ALB: Internet-facing, HTTPS (ACM cert), 180s idle timeout
- NAT Gateway: Single NAT in us-east-1a ($32/month)
- VPC Endpoints: ECR, Logs, Secrets Manager ($35/month)
- Security Groups: ALB + ECS tasks
- IAM Roles: Task execution + task roles

**Facilitator-Specific Resources**:
- ECR Repository: `karmacadabra/facilitator`
- ALB Target Group + 4 Listener Rules (path-based routing)
- CloudWatch Log Group: `/ecs/karmacadabra-prod/facilitator` (7-day retention)
- CloudWatch Alarms: High CPU, High Memory, Low Task Count, Unhealthy Targets (5 alarms)
- Route53 DNS: `facilitator.karmacadabra.ultravioletadao.xyz`, `facilitator.ultravioletadao.xyz`
- ACM Certificate: `*.ultravioletadao.xyz` (shared)

### Terraform Extraction Strategy

**Recommended Approach**: **Full Infrastructure Duplication**

**Why Duplicate Instead of Share?**
1. **Operational Independence**: Can destroy/recreate without affecting agents
2. **Zero State Dependencies**: No terraform state conflicts between repos
3. **Simpler Disaster Recovery**: Isolated blast radius
4. **Clean Namespace**: No "karmacadabra" prefixes
5. **Cost-Effective**: With optimizations, only +$4-10/month

**New Infrastructure** (to create):
```
facilitator-prod/
├── VPC: 10.1.0.0/16 (different CIDR)
├── NAT Instance: t4g.nano ($8/month vs $32 NAT Gateway)
├── ALB: Simplified (no multi-agent routing)
├── ECS Cluster: facilitator-prod
├── IAM Roles: facilitator-specific
└── Security Groups: facilitator-specific
```

**Resources to Recreate**:
- VPC + subnets (2 AZs)
- Internet Gateway + NAT (instance or gateway)
- ALB + target group (single service)
- ECS cluster + service + task definition
- IAM roles (task execution + task)
- Security groups (ALB + ECS)
- CloudWatch log group + alarms
- Route53 DNS records
- ECR repository

**Resources to Reference** (data sources):
- ACM certificate: `*.ultravioletadao.xyz` (if shared across projects)
- Route53 hosted zone: `ultravioletadao.xyz` (if managed separately)

### Cost Analysis

#### Current Cost (Shared Infrastructure)

**Facilitator Share**:
- Fargate (2 vCPU / 4 GB, 24/7): $35-45/month
- Share of VPC/NAT/ALB (1/8th of 8 services): ~$9.50/month
- **Total**: $44-54/month

**Shared Resources Cost** (split 8 ways):
- NAT Gateway: $32/month
- ALB: $16/month (base) + $6/month (LCU)
- VPC Endpoints: $35/month
- **Total Shared**: $76/month ÷ 8 services = ~$9.50/service

#### Standalone Cost (Initial - No Optimization)

- Fargate (2 vCPU / 4 GB, 24/7): $35-45/month
- ALB: $16/month (base) + $6/month (LCU) = $22/month
- NAT Gateway: $32/month
- VPC Endpoints: $35/month
- CloudWatch: $2/month
- **Total**: $126-138/month

**Increase**: +$82-84/month ❌ **TOO EXPENSIVE**

#### Standalone Cost (Optimized)

Apply these optimizations:

1. **Use NAT Instance** (-$24/month)
   - t4g.nano NAT instance: $8/month (vs $32 NAT Gateway)
   - Trade-off: Requires maintenance, 5 Gbps throughput (sufficient)

2. **Remove VPC Endpoints** (-$35/month)
   - Route ECR/Logs/Secrets Manager through NAT
   - Trade-off: +$5-10/month NAT data transfer

3. **Right-Size Fargate Task** (-$17-22/month)
   - Test 1 vCPU / 2 GB (50% reduction)
   - Monitor CPU/memory <50% for 1 week before reducing
   - Current: 2 vCPU / 4 GB = $35-45/month
   - Optimized: 1 vCPU / 2 GB = $17-22/month

**Optimized Total**: $41-51/month
**Increase**: +$4-10/month ✅ **ACCEPTABLE**

#### Cost Comparison Table

| Resource | Current (Shared) | Standalone (Initial) | Standalone (Optimized) |
|----------|------------------|----------------------|------------------------|
| Fargate | $35-45 | $35-45 | $17-22 |
| ALB | ~$2.75 (1/8th) | $22 | $22 |
| NAT | ~$4 (1/8th) | $32 | $8 (instance) |
| VPC Endpoints | ~$4.50 (1/8th) | $35 | $0 (removed) |
| CloudWatch | Included | $2 | $2 |
| **Total** | **$46-56** | **$126-138** | **$41-51** |
| **vs Current** | Baseline | +$82/mo ❌ | **+$4-10/mo ✅** |

### Terraform File Structure

**Target Structure**:
```
facilitator/terraform/
├── modules/
│   └── facilitator-service/         # Reusable module
│       ├── main.tf                  # ECS cluster, service, task
│       ├── vpc.tf                   # VPC, subnets, IGW, NAT
│       ├── alb.tf                   # Load balancer (simplified, no multi-agent routing)
│       ├── iam.tf                   # Task execution/task roles
│       ├── security_groups.tf       # ALB + ECS tasks SGs
│       ├── cloudwatch.tf            # Logs, alarms, metrics
│       ├── ecr.tf                   # Container registry
│       ├── route53.tf               # DNS records
│       ├── acm.tf                   # SSL certificate (or data source)
│       ├── variables.tf             # Input variables
│       ├── outputs.tf               # Outputs (ALB DNS, ECS cluster ARN, etc.)
│       └── README.md
├── environments/
│   ├── production/
│   │   ├── main.tf                  # Calls ../modules/facilitator-service
│   │   ├── backend.tf               # S3 backend (facilitator-terraform-state)
│   │   ├── terraform.tfvars         # Production values
│   │   └── README.md
│   ├── staging/
│   │   ├── main.tf
│   │   ├── backend.tf
│   │   ├── terraform.tfvars
│   │   └── README.md
│   └── dev/
│       ├── main.tf
│       ├── backend.tf
│       ├── terraform.tfvars         # Fargate Spot, smaller tasks
│       └── README.md
├── task-definitions/
│   ├── facilitator-mainnet.json     # Current task def (reference)
│   └── facilitator-mainnet-v2.json  # Latest task def
├── Makefile                         # Deployment commands
└── README.md                        # Infrastructure documentation
```

**Key Simplifications vs Multi-Agent**:
- ❌ No `for_each = var.agents` loops (8 agents → 1 service)
- ❌ No `each.key == "facilitator"` conditionals
- ❌ No multi-agent ALB path routing (8 rules → 1 default action)
- ❌ No agent-specific security group rules
- ✅ ~40% less terraform code
- ✅ Easier to understand and maintain

**Critical Variables**:
```hcl
# terraform/environments/production/terraform.tfvars

# COST-CRITICAL
use_fargate_spot       = false       # Facilitator needs stability (agents use true)
use_nat_instance       = true        # $8/month (vs $32 NAT Gateway)
enable_vpc_endpoints   = false       # Save $35/month, use NAT
single_nat_gateway     = true        # Single NAT (vs multi-AZ $64/month)

# NETWORK-CRITICAL
vpc_cidr               = "10.1.0.0/16"  # Different from karmacadabra (10.0.0.0/16)
availability_zones     = ["us-east-1a", "us-east-1b"]

# ALB-CRITICAL
alb_idle_timeout       = 180         # 3 minutes (Base mainnet needs >60s)

# ECS-CRITICAL (test and optimize after deployment)
task_cpu               = 1024        # 1 vCPU (start here, monitor)
task_memory            = 2048        # 2 GB (start here, monitor)
desired_count          = 1
min_capacity           = 1
max_capacity           = 3
cpu_target_value       = 75          # Auto-scale at 75% CPU
memory_target_value    = 80          # Auto-scale at 80% memory

# SECRETS-CRITICAL
evm_secret_name        = "facilitator-evm-private-key"
solana_secret_name     = "facilitator-solana-keypair"
quicknode_secret_name  = "facilitator-quicknode-base-rpc"  # Optional

# DNS-CRITICAL
domain_name            = "facilitator.ultravioletadao.xyz"
hosted_zone_name       = "ultravioletadao.xyz"

# MONITORING-CRITICAL
log_retention_days     = 7           # CloudWatch logs (increase for production)
enable_container_insights = true
```

### AWS Secrets Migration

**Current Secrets** (to migrate):
1. `karmacadabra-facilitator-mainnet` → `facilitator-evm-private-key`
2. `karmacadabra-solana-keypair` → `facilitator-solana-keypair`
3. `karmacadabra-quicknode-base-rpc` → `facilitator-quicknode-base-rpc`

**Migration Process**:
```bash
# 1. Export current secrets (secure environment only!)
aws secretsmanager get-secret-value \
  --secret-id karmacadabra-facilitator-mainnet \
  --query SecretString \
  --output text > /tmp/evm-key.json

aws secretsmanager get-secret-value \
  --secret-id karmacadabra-solana-keypair \
  --query SecretString \
  --output text > /tmp/solana-key.json

# 2. Create new secrets
aws secretsmanager create-secret \
  --name facilitator-evm-private-key \
  --description "Facilitator EVM private key for mainnet transactions" \
  --secret-string file:///tmp/evm-key.json

aws secretsmanager create-secret \
  --name facilitator-solana-keypair \
  --description "Facilitator Solana keypair for mainnet transactions" \
  --secret-string file:///tmp/solana-key.json

# 3. Securely delete temporary files
shred -vfz -n 10 /tmp/evm-key.json /tmp/solana-key.json

# 4. Update IAM task role to grant access
# (Terraform will handle this via aws_iam_role_policy)
```

**Security Notes**:
- ⚠️ Never expose private keys in logs, task definitions, or Terraform state
- ✅ Use `valueFrom` with Secrets Manager ARN in task definition
- ✅ Rotate secrets after migration completes
- ✅ Use secure workstation for secret export (not shared/dev machine)

---

## Extraction Phases

### Phase 1: Discovery & Planning (CURRENT PHASE)

**Status**: ✅ In Progress (90% complete)

**Completed**:
- [x] File inventory (all facilitator-related files identified)
- [x] Terraform infrastructure analysis
- [x] AWS deployment analysis
- [x] Cost analysis with optimization strategy
- [x] Risk assessment
- [x] Master plan document created

**Remaining**:
- [ ] User review of master plan
- [ ] Key decision approvals (see Key Decision Points section)
- [ ] Create Phase 2 execution checklist

**Deliverables**:
- This document (EXTRACTION_MASTER_PLAN.md)
- Three detailed analysis documents (terraform, AWS, file inventory)

---

### Phase 2: Pre-Extraction Setup (Estimated: 3-4 hours)

**Goal**: Prepare infrastructure and tooling before moving files

#### Task 2.1: Create Target Directory Structure

```bash
cd z:\ultravioleta\dao\karmacadabra
mkdir facilitator
cd facilitator

# Create directory structure
mkdir -p src/chain
mkdir -p static/images
mkdir -p crates/x402-axum/src
mkdir -p crates/x402-reqwest/src
mkdir -p examples/x402-axum-example/src
mkdir -p examples/x402-reqwest-example/src
mkdir -p tests/integration
mkdir -p tests/x402/payloads
mkdir -p tests/load
mkdir -p tests/fixtures
mkdir -p scripts
mkdir -p terraform/modules/facilitator-service
mkdir -p terraform/environments/production
mkdir -p terraform/environments/staging
mkdir -p terraform/task-definitions
mkdir -p docs/bug-reports
mkdir -p .github/workflows
mkdir -p .cargo
```

**Verification**: `tree facilitator/ -L 2` shows all directories

#### Task 2.2: Initialize Git Repository

**Option A**: Preserve History (Recommended)
```bash
cd z:\ultravioleta\dao\karmacadabra

# Create new branch for extraction
git checkout -b extract-facilitator

# Use git filter-branch to extract x402-rs history
git filter-branch --prune-empty --subdirectory-filter x402-rs -- --all

# Rename branch
git branch -m facilitator-main

# Export to new repo
cd facilitator
git init
git remote add source ../
git pull source facilitator-main
git remote remove source
```

**Option B**: Fresh Start (Simpler, loses history)
```bash
cd z:\ultravioleta\dao\karmacadabra\facilitator
git init
git add .
git commit -m "Initial commit: Extract facilitator from karmacadabra

Source: z:\ultravioleta\dao\karmacadabra\x402-rs
Parent repo: https://github.com/ultravioletadao/karmacadabra (if public)

This repository contains the x402-rs payment facilitator extracted from the
karmacadabra monorepo for independent deployment and maintenance.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

**Decision Required**: Choose Option A (history) or Option B (fresh)

#### Task 2.3: Set Up AWS Infrastructure Prerequisites

**S3 Backend for Terraform State**:
```bash
# Create S3 bucket for terraform state
aws s3 mb s3://facilitator-terraform-state --region us-east-1

# Enable versioning
aws s3api put-bucket-versioning \
  --bucket facilitator-terraform-state \
  --versioning-configuration Status=Enabled

# Create DynamoDB table for state locking
aws dynamodb create-table \
  --table-name facilitator-terraform-locks \
  --attribute-definitions AttributeName=LockID,AttributeType=S \
  --key-schema AttributeName=LockID,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST \
  --region us-east-1
```

**Create New ECR Repository**:
```bash
aws ecr create-repository \
  --repository-name facilitator \
  --image-scanning-configuration scanOnPush=true \
  --encryption-configuration encryptionType=AES256 \
  --region us-east-1
```

**Migrate AWS Secrets** (see Infrastructure section above for commands)

**Verification**:
- [ ] S3 bucket exists: `aws s3 ls s3://facilitator-terraform-state`
- [ ] DynamoDB table exists: `aws dynamodb describe-table --table-name facilitator-terraform-locks`
- [ ] ECR repo exists: `aws ecr describe-repositories --repository-names facilitator`
- [ ] New secrets created: `aws secretsmanager list-secrets | grep facilitator`

---

### Phase 3: File Extraction (Estimated: 4-6 hours)

**Goal**: Copy all facilitator files to new directory with correct structure

#### Task 3.1: Copy Core x402-rs Source

```bash
cd z:\ultravioleta\dao\karmacadabra

# Copy entire x402-rs/ directory
cp -r x402-rs/* facilitator/

# Verify critical files
grep -q "Ultravioleta DAO" facilitator/static/index.html && echo "✅ Branding preserved"
grep -q "include_str!" facilitator/src/handlers.rs && echo "✅ Custom handler preserved"
grep -c "Network::" facilitator/src/network.rs # Should output 17+
```

**Verification Checklist**:
- [ ] All source files copied (compare file counts: `find x402-rs -type f | wc -l`)
- [ ] Static assets intact (check `static/` directory has all 12 images)
- [ ] Cargo workspace structure preserved
- [ ] Examples directory copied
- [ ] Documentation copied

#### Task 3.2: Extract and Adapt Testing Files

```bash
# Integration tests
cp scripts/test_glue_payment_simple.py facilitator/tests/integration/test_glue_payment.py
cp scripts/test_usdc_payment_base.py facilitator/tests/integration/test_usdc_payment.py
cp scripts/test_base_usdc_stress.py facilitator/tests/integration/test_payment_stress.py
cp scripts/test_facilitator_verbose.py facilitator/tests/integration/test_facilitator.py

# X402 tests
cp -r tests/x402/* facilitator/tests/x402/

# Test fixtures
cp scripts/usdc_contracts_facilitator.json facilitator/tests/fixtures/usdc_contracts.json

# Update import paths in all test files
cd facilitator/tests
find . -name "*.py" -exec sed -i 's|../../scripts/|../scripts/|g' {} +
find . -name "*.py" -exec sed -i 's|http://localhost:8080|$FACILITATOR_URL|g' {} +
```

**Manual Edits Required**:
- Update `FACILITATOR_URL` in test files to use environment variable
- Remove karmacadabra-specific imports (if any)
- Update paths to test fixtures

**Verification**:
- [ ] All test files copied
- [ ] Import paths updated
- [ ] Test fixtures accessible
- [ ] README/TROUBLESHOOTING docs copied

#### Task 3.3: Extract Deployment Scripts

```bash
# Deployment
cp scripts/build-and-push.py facilitator/scripts/build-and-push.py
cp scripts/deploy-to-fargate.py facilitator/scripts/deploy.py
cp scripts/upgrade_facilitator.ps1 facilitator/scripts/upgrade.ps1

# Secrets management
cp scripts/setup_facilitator_secrets.py facilitator/scripts/setup_secrets.py
cp scripts/migrate_facilitator_secrets.py facilitator/scripts/migrate_secrets.py
cp scripts/rotate-facilitator-wallet.py facilitator/scripts/rotate_wallet.py

# Diagnostic
cp scripts/check_facilitator_config.py facilitator/scripts/check_config.py
cp scripts/check_facilitator_version.py facilitator/scripts/check_version.py
cp scripts/diagnose_usdc_payment.py facilitator/scripts/diagnose_payment.py
```

**Simplification Required**:
- Extract only facilitator-related code from `build-and-push.py`
- Remove multi-agent logic from `deploy.py`
- Update AWS resource names (remove "karmacadabra" prefix)

**Example**:
```python
# build-and-push.py - BEFORE (in karmacadabra)
SERVICES = {
    'facilitator': {...},
    'validator': {...},
    'karma-hello': {...},
    # ... 5 more agents
}

# build-and-push.py - AFTER (in facilitator)
SERVICE_CONFIG = {
    'name': 'facilitator',
    'context': '.',
    'dockerfile': 'Dockerfile',
    'repository': 'facilitator'  # No "karmacadabra/" prefix
}
```

#### Task 3.4: Copy Terraform Infrastructure

```bash
# Copy entire ecs-fargate directory as starting point
cp -r terraform/ecs-fargate/* facilitator/terraform/modules/facilitator-service/

# Copy task definitions
cp facilitator-task-def-mainnet-v2.json facilitator/terraform/task-definitions/
cp facilitator-task-def-mainnet.json facilitator/terraform/task-definitions/mainnet-v1.json

# Copy infrastructure docs
cp terraform/ecs-fargate/FACILITATOR_MAINNET_MIGRATION.md facilitator/terraform/
cp terraform/ecs-fargate/INFRASTRUCTURE_ANALYSIS_REPORT.md facilitator/docs/
```

**Major Simplification Required** (see Task 3.5)

#### Task 3.5: Simplify Terraform (CRITICAL TASK)

**Remove Multi-Agent Complexity**:

**BEFORE** (karmacadabra multi-agent):
```hcl
# main.tf
variable "agents" {
  type = map(object({
    name = string
    port = number
    cpu  = number
    memory = number
  }))
}

resource "aws_ecs_service" "agents" {
  for_each = var.agents
  name     = "karmacadabra-prod-${each.key}"
  # ...
}

# alb.tf
resource "aws_lb_target_group" "agents" {
  for_each = var.agents
  name     = "karmacadabra-${each.key}"
  # ...
}

resource "aws_lb_listener_rule" "agent_routing" {
  for_each = var.agents

  condition {
    path_pattern {
      values = each.key == "facilitator" ? ["/*"] : ["/api/${each.key}/*"]
    }
  }
}
```

**AFTER** (facilitator standalone):
```hcl
# main.tf
resource "aws_ecs_service" "facilitator" {
  name            = "facilitator-prod"
  cluster         = aws_ecs_cluster.facilitator.id
  task_definition = aws_ecs_task_definition.facilitator.arn
  desired_count   = var.desired_count
  # ...
}

# alb.tf
resource "aws_lb_target_group" "facilitator" {
  name     = "facilitator-prod"
  port     = 8080
  protocol = "HTTP"
  vpc_id   = aws_vpc.facilitator.id
  # ...
}

resource "aws_lb_listener_rule" "facilitator" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 100

  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.facilitator.arn
  }

  condition {
    path_pattern {
      values = ["/*"]  # All traffic goes to facilitator
    }
  }
}
```

**Changes**:
- ❌ Remove `for_each = var.agents` from ALL resources
- ❌ Remove `each.key`, `each.value` references
- ❌ Remove conditional logic for multi-agent routing
- ✅ Single service, single target group, single listener rule
- ✅ Rename resources: `karmacadabra-prod-facilitator` → `facilitator-prod`
- ✅ Update VPC CIDR: `10.0.0.0/16` → `10.1.0.0/16`

**Estimated LOC Reduction**: ~40% (from ~800 lines to ~480 lines)

**Files to Modify**:
- `main.tf` - Remove `for_each`, single ECS service
- `variables.tf` - Remove `agents` map, add single service variables
- `alb.tf` - Single target group, single listener rule
- `iam.tf` - Single task execution role, single task role
- `security_groups.tf` - Simplified rules (no inter-agent communication)
- `cloudwatch.tf` - Single log group, single alarm set

**Verification**:
- [ ] No `for_each` loops in any .tf file
- [ ] No `each.key` or `each.value` references
- [ ] All resource names use "facilitator" (not "karmacadabra")
- [ ] VPC CIDR is `10.1.0.0/16`
- [ ] `terraform validate` passes

#### Task 3.6: Copy Documentation

```bash
# Facilitator-specific docs
cp docs/FACILITATOR_TESTING.md facilitator/docs/TESTING.md
cp docs/FACILITATOR_WALLET_ROTATION.md facilitator/docs/WALLET_ROTATION.md
cp docs/X402_FORK_STRATEGY.md facilitator/docs/UPSTREAM_MERGE_STRATEGY.md
cp docs/migration/FACILITATOR_SECRETS_MIGRATION.md facilitator/docs/SECRETS_MIGRATION.md

# Bug reports
cp FACILITATOR_VALIDATION_BUG.md facilitator/docs/bug-reports/validation-bug.md
cp BASE_USDC_BUG_INVESTIGATION_REPORT.md facilitator/docs/bug-reports/base-usdc-bug.md

# Infrastructure analysis
cp AWS_FACILITATOR_INFRASTRUCTURE_EXTRACTION.md facilitator/docs/infrastructure-analysis.md
```

**Adaptation Required**:
- Remove references to karmacadabra agents
- Update paths (e.g., `z:\ultravioleta\dao\karmacadabra\x402-rs` → `facilitator/`)
- Update repository URLs (if karmacadabra is public)

#### Task 3.7: Create Configuration Files

**Docker Compose** (extract from karmacadabra):
```yaml
# facilitator/docker-compose.yml
version: '3.8'

services:
  facilitator:
    build:
      context: .
      dockerfile: Dockerfile
    container_name: facilitator
    ports:
      - "8080:8080"
    environment:
      - RUST_LOG=info
      - EVM_PRIVATE_KEY=${EVM_PRIVATE_KEY}
      - SOLANA_PRIVATE_KEY=${SOLANA_PRIVATE_KEY}
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
```

**.env.example**:
```bash
# Blockchain Keys (NEVER commit actual keys!)
EVM_PRIVATE_KEY=0x0000000000000000000000000000000000000000000000000000000000000000
SOLANA_PRIVATE_KEY=[0,0,0,0,...]  # 64-byte array

# RPC URLs (Public - can commit)
RPC_URL_BASE_MAINNET=https://mainnet.base.org
RPC_URL_BASE_SEPOLIA=https://sepolia.base.org
RPC_URL_AVALANCHE_MAINNET=https://api.avax.network/ext/bc/C/rpc
RPC_URL_AVALANCHE_FUJI=https://api.avax-test.network/ext/bc/C/rpc
# ... (17 networks total)

# Premium RPC (Optional)
QUICKNODE_BASE_RPC=https://your-endpoint.quiknode.pro/...

# OpenTelemetry (Optional)
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
RUST_LOG=info
```

**.gitignore**:
```
# Rust
target/
Cargo.lock
**/*.rs.bk

# Environment
.env
.env.local
*.pem
*.key

# AWS
.aws/
terraform.tfstate*
.terraform/

# IDE
.vscode/
.idea/
*.swp
*.swo

# OS
.DS_Store
Thumbs.db
```

**README.md** (see Phase 5 for full content)

**Verification**:
- [ ] docker-compose.yml builds and runs
- [ ] .env.example has all required variables
- [ ] .gitignore covers secrets and build artifacts
- [ ] README.md has quickstart instructions

---

### Phase 4: Testing & Validation (Estimated: 4-6 hours)

**Goal**: Verify facilitator works standalone before infrastructure migration

#### Task 4.1: Local Build Test

```bash
cd z:\ultravioleta\dao\karmacadabra\facilitator

# Clean build
cargo clean
cargo build --release

# Check binary size (should be ~50-80 MB)
ls -lh target/release/x402-rs

# Verify branding in binary (optional)
strings target/release/x402-rs | grep "Ultravioleta DAO"
```

**Success Criteria**:
- [x] Build completes without errors
- [x] Binary size is reasonable (~50-80 MB)
- [x] No warnings about missing files

#### Task 4.2: Local Runtime Test

```bash
# Set up test environment
cp .env.example .env
# Edit .env with test keys (testnet only!)

# Run facilitator
cargo run --release

# In another terminal:
# Health check
curl http://localhost:8080/health
# Expected: 200 OK

# Branding check
curl http://localhost:8080/ | grep "Ultravioleta DAO"
# Expected: Found "Ultravioleta DAO"

# Networks endpoint
curl http://localhost:8080/networks
# Expected: JSON with 17 networks
```

**Success Criteria**:
- [ ] Health endpoint returns 200
- [ ] Landing page shows Ultravioleta DAO branding
- [ ] `/networks` returns all 17 networks
- [ ] No errors in console logs

#### Task 4.3: Payment Flow Test (Testnet)

```bash
# Run payment test with Avalanche Fuji (testnet)
cd tests/integration
python test_glue_payment.py --network fuji

# Run USDC payment test with Base Sepolia
python test_usdc_payment.py --network base-sepolia

# Run stress test (100 payments)
python test_payment_stress.py --network fuji --count 100
```

**Success Criteria**:
- [ ] GLUE payment succeeds on Avalanche Fuji
- [ ] USDC payment succeeds on Base Sepolia
- [ ] Stress test completes without timeouts
- [ ] All transactions confirmed on-chain

#### Task 4.4: Docker Build Test

```bash
# Build Docker image
docker build -t facilitator:test .

# Check image size (should be ~300-500 MB)
docker images facilitator:test

# Run container
docker run -d -p 8080:8080 \
  --env-file .env \
  --name facilitator-test \
  facilitator:test

# Wait for startup
sleep 10

# Test endpoints
curl http://localhost:8080/health
curl http://localhost:8080/ | grep "Ultravioleta"

# Check logs
docker logs facilitator-test

# Stop and remove
docker stop facilitator-test
docker rm facilitator-test
```

**Success Criteria**:
- [ ] Docker image builds successfully
- [ ] Container starts without errors
- [ ] Health checks pass
- [ ] Branding preserved in Docker deployment

#### Task 4.5: Docker Compose Test

```bash
# Start with docker-compose
docker-compose up -d

# Check status
docker-compose ps

# Test endpoints
curl http://localhost:8080/health
curl http://localhost:8080/networks

# View logs
docker-compose logs -f facilitator

# Run integration test
cd tests/integration
python test_complete_flow.py --facilitator http://localhost:8080

# Stop
docker-compose down
```

**Success Criteria**:
- [ ] Service starts with docker-compose
- [ ] All endpoints accessible
- [ ] Integration tests pass
- [ ] Clean shutdown with `docker-compose down`

#### Task 4.6: Terraform Validation

```bash
cd facilitator/terraform/environments/staging

# Initialize
terraform init

# Validate
terraform validate

# Plan (dry-run)
terraform plan -out=staging.tfplan

# Review plan output
# - Should create ~40-50 resources
# - No errors or warnings
# - VPC CIDR is 10.1.0.0/16
# - ECS cluster name is "facilitator-staging"
```

**Success Criteria**:
- [ ] `terraform init` succeeds
- [ ] `terraform validate` passes
- [ ] `terraform plan` completes without errors
- [ ] Plan output shows expected resources
- [ ] No references to "karmacadabra" in resource names

---

### Phase 5: Documentation (Estimated: 3-4 hours)

**Goal**: Create comprehensive documentation for standalone usage

#### Task 5.1: Main README.md

**Required Sections**:
1. **Project Overview**
   - What is x402-rs facilitator
   - Supported networks (17 networks table)
   - Key features (gasless payments, HTTP 402, multi-chain)

2. **Quick Start**
   - Prerequisites (Rust, Docker, AWS CLI)
   - Environment setup (`.env` configuration)
   - Local development (`cargo run`)
   - Docker deployment (`docker-compose up`)

3. **Deployment**
   - AWS infrastructure setup (Terraform)
   - Secrets management (AWS Secrets Manager)
   - Production deployment checklist

4. **API Documentation**
   - Endpoints (`/health`, `/networks`, `/settle`, `/pay`)
   - Request/response formats
   - Error codes

5. **Network Configuration**
   - How to add new networks
   - RPC endpoint configuration
   - USDC contract addresses per network

6. **Testing**
   - Running integration tests
   - Payment flow testing
   - Load testing

7. **Maintenance**
   - Wallet rotation
   - Upstream merges from x402-rs
   - Monitoring and alerting

8. **Troubleshooting**
   - Common issues
   - Debug mode
   - Log analysis

**Example Structure**:
```markdown
# x402-rs Payment Facilitator

> Multi-chain payment facilitator supporting gasless micropayments via HTTP 402 protocol

## Features

- 🌐 **17 Blockchain Networks** (7 mainnets + 10 testnets)
- ⚡ **Gasless Payments** via EIP-3009 transferWithAuthorization
- 🔒 **Trustless** - No custody, users sign authorizations off-chain
- 🚀 **High Performance** - Rust + Axum, handles 100+ tx/sec
- 📊 **Production Ready** - Deployed on AWS ECS, monitored with CloudWatch

## Supported Networks

| Network | Mainnet | Testnet | Token |
|---------|---------|---------|-------|
| Base | ✅ | ✅ Base Sepolia | USDC |
| Avalanche | ✅ | ✅ Fuji | USDC |
| Celo | ✅ | ✅ Alfajores | cUSD |
| ... | ... | ... | ... |

## Quick Start

### Local Development

```bash
# Clone repository
git clone [repo-url]
cd facilitator

# Configure environment
cp .env.example .env
# Edit .env with your keys (testnet recommended)

# Run facilitator
cargo run --release

# Test
curl http://localhost:8080/health
```

[... rest of README]
```

#### Task 5.2: DEPLOYMENT.md

**Required Sections**:
1. **AWS Prerequisites**
   - AWS account setup
   - IAM permissions required
   - S3 backend setup
   - Secrets Manager setup

2. **Terraform Deployment**
   - Initialize backend
   - Configure variables
   - Apply infrastructure
   - Verify deployment

3. **Docker Image**
   - Build and push to ECR
   - Tag strategy
   - Rollback procedures

4. **Zero-Downtime Updates**
   - Blue-green deployment
   - Rolling updates
   - Health check configuration

5. **Post-Deployment**
   - DNS configuration
   - SSL certificate validation
   - Load testing

#### Task 5.3: TESTING.md

**Required Sections**:
1. **Test Suite Overview**
   - Integration tests
   - Load tests
   - Security tests

2. **Running Tests**
   - Prerequisites
   - Test environment setup
   - Running specific test suites

3. **Payment Flow Tests**
   - GLUE token tests (Avalanche)
   - USDC tests (Base, Polygon, etc.)
   - Multi-network tests

4. **Load Testing**
   - k6 configuration
   - Artillery configuration
   - Performance benchmarks

#### Task 5.4: UPSTREAM_MERGE_STRATEGY.md

**Critical Document** (based on CLAUDE.md incident prevention):

**Required Sections**:
1. **Never Use Mass File Copy**
   - Why `cp -r upstream/* x402-rs/` is forbidden
   - Incident history reference

2. **Git Merge Strategy**
   - Create upstream tracking branch
   - Surgical merge process
   - Conflict resolution guidelines

3. **Protected Files**
   - Tier 1: NEVER overwrite (static/, Dockerfile)
   - Tier 2: Merge with care (handlers.rs, network.rs)
   - Tier 3: Safe to upgrade

4. **Verification Checklist**
   - Branding verification steps
   - Custom handler verification
   - Network configuration verification

5. **Rollback Procedures**
   - Git revert strategy
   - Backup restoration
   - Emergency deployment

#### Task 5.5: CONTRIBUTING.md

**Required Sections**:
1. **Development Setup**
2. **Code Style**
3. **Testing Requirements**
4. **Pull Request Process**
5. **Release Process**

#### Task 5.6: LICENSE

**Decision Required**: Choose license
- MIT (permissive, recommended for infrastructure)
- Apache 2.0 (permissive with patent grant)
- GPL v3 (copyleft)

**Note**: Check if upstream x402-rs has license requirements

---

### Phase 6: Infrastructure Migration (Estimated: 8-12 hours)

**Goal**: Deploy standalone facilitator to AWS and migrate DNS

#### Task 6.1: Deploy Staging Environment

```bash
cd facilitator/terraform/environments/staging

# Initialize backend
terraform init

# Apply infrastructure
terraform apply

# Expected output:
# - VPC, subnets, NAT, ALB created
# - ECS cluster and service created
# - ~45 resources created

# Get ALB DNS
terraform output alb_dns_name
# Example: facilitator-staging-alb-1234567890.us-east-1.elb.amazonaws.com
```

**Verification**:
- [ ] Terraform apply completes without errors
- [ ] All resources created successfully
- [ ] ALB DNS resolves
- [ ] ECS service shows 1 running task

#### Task 6.2: Build and Push Docker Image

```bash
cd facilitator

# Login to ECR
aws ecr get-login-password --region us-east-1 | \
  docker login --username AWS --password-stdin \
  518898403364.dkr.ecr.us-east-1.amazonaws.com

# Build image
docker build -t facilitator:staging-v1.0.0 .

# Tag for ECR
docker tag facilitator:staging-v1.0.0 \
  518898403364.dkr.ecr.us-east-1.amazonaws.com/facilitator:staging-v1.0.0

# Push
docker push 518898403364.dkr.ecr.us-east-1.amazonaws.com/facilitator:staging-v1.0.0

# Update ECS service (force new deployment)
aws ecs update-service \
  --cluster facilitator-staging \
  --service facilitator-staging \
  --force-new-deployment \
  --region us-east-1
```

**Verification**:
- [ ] Docker image pushed to ECR
- [ ] ECS service updated
- [ ] New task started successfully
- [ ] Old task drained

#### Task 6.3: Test Staging Deployment

```bash
# Get ALB DNS from terraform output
STAGING_URL=$(cd terraform/environments/staging && terraform output -raw alb_dns_name)

# Health check
curl https://$STAGING_URL/health

# Branding check
curl https://$STAGING_URL/ | grep "Ultravioleta DAO"

# Networks check
curl https://$STAGING_URL/networks | jq '.networks | length'
# Expected: 17

# Run integration tests against staging
cd tests/integration
export FACILITATOR_URL=https://$STAGING_URL
python test_glue_payment.py --network fuji
python test_usdc_payment.py --network base-sepolia
```

**Success Criteria**:
- [ ] All health checks pass
- [ ] Branding displays correctly
- [ ] All 17 networks available
- [ ] Payment tests succeed

#### Task 6.4: Load Test Staging

```bash
cd tests/load

# Run k6 load test (100 VUs, 5 minutes)
k6 run --vus 100 --duration 5m \
  -e FACILITATOR_URL=https://$STAGING_URL \
  k6_load_test.js

# Monitor CloudWatch metrics during test
aws cloudwatch get-metric-statistics \
  --namespace AWS/ECS \
  --metric-name CPUUtilization \
  --dimensions Name=ServiceName,Value=facilitator-staging \
  --start-time $(date -u -d '10 minutes ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 60 \
  --statistics Average,Maximum
```

**Success Criteria**:
- [ ] Load test completes without errors
- [ ] Average response time <500ms
- [ ] Error rate <1%
- [ ] CPU utilization <75%
- [ ] Memory utilization <80%
- [ ] No task restarts during test

**If Performance Issues**:
- Increase task size (1 vCPU/2 GB → 2 vCPU/4 GB)
- Enable auto-scaling
- Optimize RPC endpoint selection

#### Task 6.5: Deploy Production Environment

**Pre-Deployment Checklist**:
- [ ] Staging tests passed
- [ ] Load tests passed
- [ ] Secrets migrated to production
- [ ] Team notified of deployment
- [ ] Rollback plan documented

```bash
cd facilitator/terraform/environments/production

# Review variables
cat terraform.tfvars

# Plan deployment
terraform plan -out=prod.tfplan

# Review plan carefully (should create ~45 resources)
terraform show prod.tfplan

# Apply (WARNING: Creates production resources)
terraform apply prod.tfplan

# Get production ALB DNS
PROD_ALB_DNS=$(terraform output -raw alb_dns_name)
echo "Production ALB: $PROD_ALB_DNS"
```

**Verification**:
- [ ] Production infrastructure created
- [ ] ECS service running
- [ ] ALB health checks passing
- [ ] CloudWatch logs streaming

#### Task 6.6: Deploy Production Docker Image

```bash
cd facilitator

# Build production image
docker build -t facilitator:v1.0.0 .

# Tag for ECR
docker tag facilitator:v1.0.0 \
  518898403364.dkr.ecr.us-east-1.amazonaws.com/facilitator:v1.0.0

docker tag facilitator:v1.0.0 \
  518898403364.dkr.ecr.us-east-1.amazonaws.com/facilitator:latest

# Push both tags
docker push 518898403364.dkr.ecr.us-east-1.amazonaws.com/facilitator:v1.0.0
docker push 518898403364.dkr.ecr.us-east-1.amazonaws.com/facilitator:latest

# Update production service
aws ecs update-service \
  --cluster facilitator-prod \
  --service facilitator-prod \
  --force-new-deployment \
  --region us-east-1

# Monitor deployment
watch -n 5 'aws ecs describe-services \
  --cluster facilitator-prod \
  --services facilitator-prod \
  --region us-east-1 \
  --query "services[0].deployments"'
```

**Wait for Deployment**:
- New tasks start: ~2-3 minutes
- Health checks pass: ~1 minute
- Old tasks drain: ~3 minutes (ALB deregistration delay)
- **Total**: ~6-7 minutes

**Verification**:
- [ ] Deployment shows "PRIMARY" status
- [ ] Running count matches desired count
- [ ] Health checks passing
- [ ] No errors in CloudWatch logs

#### Task 6.7: DNS Migration (CRITICAL - Production Impact)

**Current DNS**:
```
facilitator.ultravioletadao.xyz → karmacadabra-prod-alb-xxx.elb.amazonaws.com
```

**Target DNS**:
```
facilitator.ultravioletadao.xyz → facilitator-prod-alb-yyy.elb.amazonaws.com
```

**Strategy**: Weighted routing for gradual cutover

**Step 1: Add New ALB (10% traffic)**
```bash
# Get new ALB DNS
NEW_ALB_DNS=$(cd facilitator/terraform/environments/production && \
  terraform output -raw alb_dns_name)

# Update Route53 with weighted routing
aws route53 change-resource-record-sets \
  --hosted-zone-id Z0XXXXXXXXXXXXXXX \
  --change-batch '{
    "Changes": [
      {
        "Action": "CREATE",
        "ResourceRecordSet": {
          "Name": "facilitator.ultravioletadao.xyz",
          "Type": "A",
          "SetIdentifier": "facilitator-new",
          "Weight": 10,
          "AliasTarget": {
            "HostedZoneId": "Z35SXDOTRQ7X7K",
            "DNSName": "'$NEW_ALB_DNS'",
            "EvaluateTargetHealth": true
          }
        }
      },
      {
        "Action": "UPSERT",
        "ResourceRecordSet": {
          "Name": "facilitator.ultravioletadao.xyz",
          "Type": "A",
          "SetIdentifier": "facilitator-old",
          "Weight": 90,
          "AliasTarget": {
            "HostedZoneId": "Z35SXDOTRQ7X7K",
            "DNSName": "karmacadabra-prod-alb-xxx.elb.amazonaws.com",
            "EvaluateTargetHealth": true
          }
        }
      }
    ]
  }'
```

**Monitor for 30 minutes**:
```bash
# Check error rates on new ALB
aws cloudwatch get-metric-statistics \
  --namespace AWS/ApplicationELB \
  --metric-name HTTPCode_Target_5XX_Count \
  --dimensions Name=LoadBalancer,Value=app/facilitator-prod-alb/xxx \
  --start-time $(date -u -d '30 minutes ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 300 \
  --statistics Sum

# Check agent connectivity (should still work)
curl https://validator.karmacadabra.ultravioletadao.xyz/health
curl https://karma-hello.karmacadabra.ultravioletadao.xyz/health
```

**Step 2: Increase to 50% traffic**
```bash
# Update weights (if no errors in Step 1)
# facilitator-new: 50
# facilitator-old: 50

# Monitor for 30 minutes
```

**Step 3: Full cutover (100% traffic)**
```bash
# Update weights
# facilitator-new: 100
# facilitator-old: 0 (or delete)

# Monitor for 2 hours

# If stable, delete old record set
aws route53 change-resource-record-sets \
  --hosted-zone-id Z0XXXXXXXXXXXXXXX \
  --change-batch '{
    "Changes": [
      {
        "Action": "DELETE",
        "ResourceRecordSet": {
          "Name": "facilitator.ultravioletadao.xyz",
          "Type": "A",
          "SetIdentifier": "facilitator-old",
          "Weight": 0,
          "AliasTarget": {
            "HostedZoneId": "Z35SXDOTRQ7X7K",
            "DNSName": "karmacadabra-prod-alb-xxx.elb.amazonaws.com",
            "EvaluateTargetHealth": true
          }
        }
      }
    ]
  }'
```

**Rollback (if issues occur)**:
```bash
# Immediately revert to old ALB (100%)
aws route53 change-resource-record-sets \
  --hosted-zone-id Z0XXXXXXXXXXXXXXX \
  --change-batch '{
    "Changes": [
      {
        "Action": "UPSERT",
        "ResourceRecordSet": {
          "Name": "facilitator.ultravioletadao.xyz",
          "Type": "A",
          "SetIdentifier": "facilitator-old",
          "Weight": 100,
          "AliasTarget": {
            "HostedZoneId": "Z35SXDOTRQ7X7K",
            "DNSName": "karmacadabra-prod-alb-xxx.elb.amazonaws.com",
            "EvaluateTargetHealth": true
          }
        }
      },
      {
        "Action": "UPSERT",
        "ResourceRecordSet": {
          "Name": "facilitator.ultravioletadao.xyz",
          "Type": "A",
          "SetIdentifier": "facilitator-new",
          "Weight": 0,
          "AliasTarget": {
            "HostedZoneId": "Z35SXDOTRQ7X7K",
            "DNSName": "'$NEW_ALB_DNS'",
            "EvaluateTargetHealth": true
          }
        }
      }
    ]
  }'

# DNS propagation: ~60 seconds (TTL)
```

**Success Criteria**:
- [ ] DNS resolves to new ALB
- [ ] All agents can reach facilitator
- [ ] Payment tests succeed
- [ ] No 5xx errors in CloudWatch
- [ ] Response times <500ms

---

### Phase 7: Cleanup & Finalization (Estimated: 2-3 hours)

**Goal**: Remove facilitator from karmacadabra, update documentation

#### Task 7.1: Remove Facilitator from Karmacadabra Terraform

**IMPORTANT**: Only do this AFTER DNS migration is complete and stable

```bash
cd z:\ultravioleta\dao\karmacadabra\terraform\ecs-fargate

# Backup current state
cp terraform.tfstate terraform.tfstate.backup

# Remove facilitator from agents map
# Edit variables.tf:
variable "agents" {
  type = map(object({
    name = string
    port = number
    # ...
  }))
  default = {
    # Remove "facilitator" entry
    "validator" = { ... },
    "karma-hello" = { ... },
    "abracadabra" = { ... },
    # ... other agents
  }
}

# Plan removal
terraform plan -out=remove-facilitator.tfplan

# Review plan (should destroy facilitator resources only)
terraform show remove-facilitator.tfplan

# Apply (WARNING: Destroys facilitator resources in karmacadabra)
terraform apply remove-facilitator.tfplan
```

**Resources to be Destroyed** (verify in plan):
- ECS service: `karmacadabra-prod-facilitator`
- ECS task definition family
- ALB target group for facilitator
- ALB listener rules for facilitator (4 rules)
- CloudWatch log group: `/ecs/karmacadabra-prod/facilitator`
- CloudWatch alarms (5 alarms)

**Resources to KEEP** (shared with other agents):
- VPC, subnets, NAT, IGW
- ALB (other agents still use it)
- ECS cluster
- IAM roles (if not facilitator-specific)
- Security groups (if shared)

**Verification**:
- [ ] Only facilitator resources destroyed
- [ ] Other agent services still running
- [ ] Agent health checks passing
- [ ] No errors in other agent logs

#### Task 7.2: Update Karmacadabra Documentation

**Files to Update**:

**README.md**:
```markdown
# Karmacadabra

## Architecture

**Layer 1 - Blockchain**: Avalanche Fuji (GLUE token, ERC-8004 registries)

**Layer 2 - Payment Facilitator**: [Standalone Repository](https://github.com/ultravioletadao/facilitator)
- See facilitator repo for deployment and configuration

**Layer 3 - AI Agents**: karma-hello, abracadabra, validator, skill-extractor, voice-extractor
```

**CLAUDE.md**:
- Keep x402-rs upgrade section (historical reference)
- Add note: "x402-rs facilitator extracted to separate repository on [date]"

**docker-compose.yml**:
```yaml
# Remove facilitator service
# Add external_links if agents need to reference it
services:
  karma-hello:
    # ...
    environment:
      - FACILITATOR_URL=https://facilitator.ultravioletadao.xyz  # External facilitator
```

**docs/ARCHITECTURE.md**:
- Update Layer 2 section to reference external facilitator
- Update architecture diagrams (remove x402-rs from repo structure)

#### Task 7.3: Update Facilitator Repository

**Add GitHub Repository** (if hosting on GitHub):
```bash
cd z:\ultravioleta\dao\karmacadabra\facilitator

# Add remote
git remote add origin https://github.com/ultravioletadao/facilitator.git

# Push all branches
git push -u origin --all

# Push tags
git push -u origin --tags

# Set default branch
git branch -M main
git push -u origin main
```

**Create Release** (GitHub):
- Tag: `v1.0.0`
- Title: "Initial Release - Extracted from Karmacadabra"
- Description: Summarize extraction, link to parent repo
- Attach binaries (optional)

**Update Repository Settings**:
- Add description: "Multi-chain payment facilitator supporting HTTP 402 protocol"
- Add topics: `rust`, `blockchain`, `payments`, `eip-3009`, `http-402`, `ethereum`, `solana`
- Enable GitHub Actions (if using CI/CD)
- Configure branch protection for `main`

#### Task 7.4: Final Verification

**Facilitator Repository**:
- [ ] All files present and correct
- [ ] Git history preserved (if Option A chosen)
- [ ] No references to karmacadabra in code (except docs)
- [ ] All tests passing
- [ ] Documentation complete
- [ ] CI/CD pipelines working (if configured)

**Karmacadabra Repository**:
- [ ] Facilitator references updated to external repo/URL
- [ ] No broken links in documentation
- [ ] Agents still connect to facilitator successfully
- [ ] Docker compose works without facilitator service

**Production**:
- [ ] Facilitator running on standalone infrastructure
- [ ] DNS pointing to new ALB
- [ ] All agents connecting successfully
- [ ] Payment flows working end-to-end
- [ ] CloudWatch alarms configured and silent
- [ ] No errors in past 24 hours

**Cost Verification**:
- [ ] AWS bill shows new facilitator resources
- [ ] Optimizations applied (NAT instance, right-sized tasks)
- [ ] Cost within expected range ($41-51/month)

---

## Key Decision Points

**User decisions required before proceeding:**

### Decision 1: Git History Preservation

**Options**:
- **A**: Preserve history via `git filter-branch` (recommended)
  - Pros: Full commit history, attribution, audit trail
  - Cons: Complex, larger repo size, ~2 hours extra work

- **B**: Fresh start with reference to parent
  - Pros: Clean slate, simpler, smaller repo
  - Cons: Loses history, harder to track changes

**Recommendation**: **Option A** (preserve history) for audit trail and upstream merge tracking

**Your Decision**: [ ] Option A  [ ] Option B

---

### Decision 2: Terraform Infrastructure Strategy

**Options**:
- **A**: Full infrastructure duplication (recommended)
  - New VPC, NAT, ALB, ECS cluster
  - Cost: +$4-10/month (optimized)
  - Pros: Complete independence, no state conflicts, clean namespace

- **B**: Shared VPC, new ECS cluster + ALB
  - Use existing karmacadabra VPC
  - Cost: +$0-5/month
  - Pros: Lower cost, simpler networking
  - Cons: Terraform state dependencies, coupled infrastructure

**Recommendation**: **Option A** (full duplication) for operational independence

**Your Decision**: [ ] Option A  [ ] Option B

---

### Decision 3: Cost Optimization Level

**Options**:
- **A**: Aggressive optimization (~$41-51/month)
  - NAT instance (t4g.nano)
  - No VPC endpoints (use NAT for AWS API calls)
  - Right-sized tasks (1 vCPU / 2 GB)
  - Pros: Lowest cost
  - Cons: Slightly higher latency for AWS API calls

- **B**: Balanced optimization (~$65-75/month)
  - NAT Gateway (standard)
  - VPC endpoints for ECR + Logs
  - Right-sized tasks (1 vCPU / 2 GB)
  - Pros: Better performance, managed NAT
  - Cons: Higher cost

- **C**: No optimization (~$126-138/month)
  - NAT Gateway
  - All VPC endpoints
  - Current task size (2 vCPU / 4 GB)
  - Pros: Maximum performance and reliability
  - Cons: Significantly higher cost

**Recommendation**: **Option A** (aggressive) - facilitator traffic is low, cost savings justified

**Your Decision**: [ ] Option A  [ ] Option B  [ ] Option C

---

### Decision 4: DNS Cutover Strategy

**Options**:
- **A**: Gradual cutover with weighted routing (recommended)
  - 10% → 50% → 100% over 2-3 hours
  - Instant rollback capability
  - Pros: Safe, testable, reversible
  - Cons: Requires monitoring, longer migration

- **B**: Blue-green with instant cutover
  - Test new ALB thoroughly
  - Switch DNS 100% at once
  - Pros: Faster migration, clear success/fail
  - Cons: Higher risk, requires immediate rollback if issues

**Recommendation**: **Option A** (gradual) - production stability is critical

**Your Decision**: [ ] Option A  [ ] Option B

---

### Decision 5: Repository Naming

**Options**:
- **A**: `facilitator` (simple)
- **B**: `x402-rs-facilitator` (descriptive)
- **C**: `karmacadabra-facilitator` (maintain context)
- **D**: `ultravioleta-facilitator` (DAO branding)

**Recommendation**: **Option A** (`facilitator`) - clean, simple, generic

**Your Decision**: [ ] Option A  [ ] Option B  [ ] Option C  [ ] Option D

---

### Decision 6: Cutover Timeline

**When to execute Phase 6 (production migration)?**

**Considerations**:
- Agent dependencies: All agents use facilitator for payments
- Live stream schedule: Avoid migrations during scheduled streams
- Team availability: Need monitoring for 2-3 hours post-cutover
- Backup window: Best to have recent karmacadabra backups

**Options**:
- **A**: Next week (November 8-15)
- **B**: Two weeks (November 15-22)
- **C**: Specific date: _______________

**Your Decision**: _______________

---

### Decision 7: License

**Options**:
- **A**: MIT (recommended)
- **B**: Apache 2.0
- **C**: GPL v3
- **D**: Match upstream x402-rs license (check polyphene/x402-rs)

**Recommendation**: **Option D** (match upstream) to maintain compatibility

**Your Decision**: [ ] Option A  [ ] Option B  [ ] Option C  [ ] Option D

---

## Risk Assessment

### Critical Risks (High Impact, Medium-High Probability)

#### Risk 1: Branding Assets Lost During Extraction

**Impact**: HIGH - Public-facing landing page broken on live streams
**Probability**: MEDIUM (has happened before)
**Mitigation**:
- Triple verification of static/ directory after every copy
- Automated test: `grep -q "Ultravioleta DAO" facilitator/static/index.html`
- Manual inspection of landing page before deployment
- Git commit after extraction with full diff review

**Detection**: Curl landing page shows plain text instead of HTML
**Rollback**: Restore static/ from karmacadabra repo, rebuild Docker image

---

#### Risk 2: DNS Cutover Causes Agent Downtime

**Impact**: HIGH - All agents lose payment capability
**Probability**: LOW (weighted routing mitigates)
**Mitigation**:
- Gradual cutover (10% → 50% → 100%)
- Instant rollback via weighted routing (change weight to 100 old, 0 new)
- Pre-cutover testing of new ALB with direct DNS
- Monitor agent error rates during cutover

**Detection**: Agent health checks fail, payment errors in logs
**Rollback**: Revert DNS weights to old ALB (60s propagation)

---

#### Risk 3: Terraform State Corruption

**Impact**: HIGH - Cannot manage infrastructure with Terraform
**Probability**: LOW (if following procedures)
**Mitigation**:
- S3 versioning enabled on state bucket
- DynamoDB locking prevents concurrent modifications
- Backup state file before every terraform apply
- Separate state files (karmacadabra vs facilitator)

**Detection**: Terraform plan shows unexpected changes
**Rollback**: Restore state file from S3 version history

---

#### Risk 4: AWS Secrets Not Accessible

**Impact**: CRITICAL - Facilitator cannot sign transactions
**Probability**: MEDIUM (easy to misconfigure IAM)
**Mitigation**:
- Create secrets BEFORE deploying ECS service
- Test IAM role permissions with AWS CLI before deployment
- Use same secret structure as karmacadabra
- Verify secrets in task definition JSON before ECS update

**Detection**: Container fails to start, logs show "Secret not found"
**Rollback**: Fix IAM policy, force new ECS deployment

---

### Medium Risks (Medium Impact, Low-Medium Probability)

#### Risk 5: Cost Overruns

**Impact**: MEDIUM - Higher than expected AWS bill
**Probability**: MEDIUM (depends on optimization choices)
**Mitigation**:
- Set AWS Budgets alert at $60/month
- Monitor CloudWatch billing metrics daily for first week
- Right-size tasks after 1 week of metrics
- Use Fargate on-demand (not Spot) for stability

**Detection**: AWS bill exceeds $60/month
**Mitigation**: Apply aggressive optimizations, reduce task size

---

#### Risk 6: Incomplete Test Coverage

**Impact**: MEDIUM - Bugs deployed to production
**Probability**: MEDIUM (large extraction, easy to miss edge cases)
**Mitigation**:
- Run all integration tests in staging
- Load test with k6 (100 VUs, 5 minutes)
- Test all 17 networks before production
- Monitor CloudWatch errors for 24 hours in staging

**Detection**: Errors in production logs, payment failures
**Rollback**: Revert DNS to old facilitator, fix bugs, redeploy

---

#### Risk 7: Upstream x402-rs Merge Conflicts

**Impact**: MEDIUM - Cannot merge future upstream updates
**Probability**: MEDIUM (depends on customization depth)
**Mitigation**:
- Document all customizations in CUSTOMIZATIONS.md
- Maintain upstream tracking branch
- Use git merge (not mass copy) for updates
- Test upstream merges in dev branch first

**Detection**: Git merge shows conflicts in critical files
**Resolution**: Manual conflict resolution, prioritize custom branding

---

#### Risk 8: Missing Dependencies in Extraction

**Impact**: MEDIUM - Facilitator fails to build/run standalone
**Probability**: LOW (comprehensive file inventory performed)
**Mitigation**:
- Test local build immediately after extraction (Phase 4)
- Test Docker build before infrastructure deployment
- Verify all test files copied
- Check for hardcoded paths to karmacadabra directories

**Detection**: Build errors, missing file errors
**Resolution**: Copy missing files from karmacadabra, update paths

---

### Low Risks (Low Impact or Low Probability)

#### Risk 9: Git History Loss

**Impact**: LOW - Inconvenient but not critical
**Probability**: LOW (if Option A chosen)
**Mitigation**: Use git filter-branch, test on branch first
**Resolution**: History preserved in karmacadabra repo as fallback

#### Risk 10: Documentation Outdated

**Impact**: LOW - Confusion for contributors
**Probability**: MEDIUM (large documentation set)
**Mitigation**: Update docs as Phase 5 task, peer review
**Resolution**: Incremental updates post-extraction

---

## Success Criteria

### Technical Success Criteria

**Build & Deploy**:
- [ ] Facilitator builds successfully standalone (`cargo build --release`)
- [ ] Docker image builds successfully
- [ ] Terraform deploys all infrastructure without errors
- [ ] ECS service starts and passes health checks

**Functionality**:
- [ ] Landing page displays Ultravioleta DAO branding
- [ ] All 17 networks available via `/networks` endpoint
- [ ] Payment flow succeeds on all mainnets
- [ ] Payment flow succeeds on all testnets

**Performance**:
- [ ] Average response time <500ms (99th percentile <2s)
- [ ] Error rate <1% under normal load
- [ ] Handles 100+ concurrent payments without degradation
- [ ] CPU utilization <75% under normal load
- [ ] Memory utilization <80% under normal load

**Integration**:
- [ ] All karmacadabra agents connect to facilitator successfully
- [ ] Payment authorizations verified correctly
- [ ] Transactions confirmed on-chain
- [ ] No errors in agent logs related to facilitator

**Infrastructure**:
- [ ] AWS infrastructure deployed successfully
- [ ] DNS resolves to new ALB
- [ ] SSL certificate valid
- [ ] CloudWatch logs streaming
- [ ] CloudWatch alarms configured and not firing

**Cost**:
- [ ] Monthly AWS cost $41-51 (optimized) or as per chosen option
- [ ] No unexpected resource charges
- [ ] Auto-scaling working correctly (scales up/down based on load)

---

### Operational Success Criteria

**Documentation**:
- [ ] README.md complete with quickstart
- [ ] DEPLOYMENT.md has step-by-step AWS instructions
- [ ] TESTING.md covers all test scenarios
- [ ] UPSTREAM_MERGE_STRATEGY.md prevents repeat incidents
- [ ] CONTRIBUTING.md has development guidelines

**Monitoring**:
- [ ] CloudWatch alarms for high CPU, high memory, low task count
- [ ] CloudWatch dashboard for facilitator metrics
- [ ] Log aggregation working (7-day retention)
- [ ] Error tracking configured

**Security**:
- [ ] Private keys in AWS Secrets Manager (not in code/env)
- [ ] IAM roles follow least privilege
- [ ] Security groups properly configured
- [ ] No secrets exposed in logs or task definitions

**Maintainability**:
- [ ] Terraform code simplified (no multi-agent complexity)
- [ ] All files in logical directory structure
- [ ] No hardcoded values (use variables)
- [ ] Clear separation of environments (prod/staging/dev)

---

### Project Success Criteria

**Independence**:
- [ ] Facilitator repository has no dependencies on karmacadabra
- [ ] Can deploy facilitator without karmacadabra context
- [ ] Other projects can use facilitator as standalone service

**Backward Compatibility**:
- [ ] Karmacadabra agents work without code changes
- [ ] API contracts preserved (same endpoints, same payloads)
- [ ] No breaking changes to x402 protocol

**Team Confidence**:
- [ ] Team can deploy updates independently
- [ ] Team can rollback if needed
- [ ] Team understands architecture and risks
- [ ] Documentation sufficient for new contributors

**Zero Downtime**:
- [ ] Production cutover completes without agent errors
- [ ] No payment failures during migration
- [ ] DNS propagation <5 minutes
- [ ] Rollback tested and working

---

## Appendix: Commands Reference

### Quick Start Commands

```bash
# Local development
cd facilitator
cargo run --release

# Docker
docker-compose up -d

# Deploy staging
cd terraform/environments/staging
terraform init && terraform apply

# Deploy production
cd terraform/environments/production
terraform init && terraform apply

# Build and push Docker
scripts/build-and-push.sh v1.0.0

# Run tests
cd tests/integration
python test_glue_payment.py --network fuji
```

### Monitoring Commands

```bash
# ECS service status
aws ecs describe-services \
  --cluster facilitator-prod \
  --services facilitator-prod \
  --query 'services[0].{status:status,running:runningCount,desired:desiredCount}'

# CloudWatch logs
aws logs tail /ecs/facilitator-prod --follow

# Metrics
aws cloudwatch get-metric-statistics \
  --namespace AWS/ECS \
  --metric-name CPUUtilization \
  --dimensions Name=ServiceName,Value=facilitator-prod \
  --start-time $(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%S) \
  --end-time $(date -u +%Y-%m-%dT%H:%M:%S) \
  --period 300 \
  --statistics Average,Maximum
```

### Rollback Commands

```bash
# Revert DNS (immediate)
aws route53 change-resource-record-sets \
  --hosted-zone-id Z0XXXXXXXXXXXXXXX \
  --change-batch file://revert-dns.json

# Revert ECS task definition
aws ecs update-service \
  --cluster facilitator-prod \
  --service facilitator-prod \
  --task-definition facilitator-prod:PREVIOUS_REVISION

# Restore from backup
git checkout HEAD~1 -- facilitator/
```

---

## Next Steps

1. **Review this master plan** with the team
2. **Make key decisions** (see Key Decision Points section)
3. **Schedule cutover window** (see Decision 6)
4. **Begin Phase 2** (Pre-Extraction Setup) once approved
5. **Create tracking board** (optional - use GitHub Projects or Jira)

**Estimated Total Timeline**: 3-4 weeks calendar time, 29-41 hours effort

---

**Document Status**: ✅ Ready for Review
**Next Action**: User decisions on Key Decision Points

**Questions or Concerns**: Add comments to this document or discuss with team before proceeding.

