# Terraform Changes Required for x402 v2 Migration

**Quick Reference:** Exact code changes needed

---

## File 1: variables.tf

**Location:** `terraform/environments/production/variables.tf`

**Add at end of file (after line 151):**

```hcl
variable "x402_version_support" {
  description = "Supported x402 protocol versions (comma-separated: v1, v2, or v1,v2)"
  type        = string
  default     = "v1,v2"  # Dual support during migration

  validation {
    condition     = can(regex("^(v1|v2|v1,v2|v2,v1)$", var.x402_version_support))
    error_message = "Must be one of: v1, v2, v1,v2, or v2,v1"
  }
}
```

---

## File 2: main.tf

**Location:** `terraform/environments/production/main.tf`

**Change 1: Add environment variable to task definition**

Find the `environment` block around **line 479**:

**BEFORE:**
```hcl
environment = [
  {
    name  = "RUST_LOG"
    value = "info"
  },
  {
    name  = "SIGNER_TYPE"
    value = "private-key"
  },
  {
    name  = "PORT"
    value = "8080"
  },
```

**AFTER:**
```hcl
environment = [
  {
    name  = "RUST_LOG"
    value = "info"
  },
  {
    name  = "X402_VERSION_SUPPORT"  # NEW LINE
    value = var.x402_version_support  # NEW LINE
  },  # NEW LINE
  {
    name  = "SIGNER_TYPE"
    value = "private-key"
  },
  {
    name  = "PORT"
    value = "8080"
  },
```

**That's it!** Only one 3-line addition to `main.tf`.

---

## File 3: cloudwatch-v2-metrics.tf (NEW FILE)

**Location:** `terraform/environments/production/cloudwatch-v2-metrics.tf`

**Action:** File already created. Contains:
- 7 CloudWatch metric filters
- 3 CloudWatch alarms
- 1 CloudWatch dashboard

**No changes needed** - ready to apply.

---

## File 4: outputs.tf (Optional Enhancement)

**Location:** `terraform/environments/production/outputs.tf`

**Add at end of file:**

```hcl
output "x402_version_support" {
  description = "Supported x402 protocol versions"
  value       = var.x402_version_support
}
```

This allows you to verify the setting with `terraform output`.

---

## Summary of Changes

| File | Lines Changed | Type | Risk |
|------|---------------|------|------|
| `variables.tf` | +11 | New variable | Low |
| `main.tf` | +3 | Environment var | Low |
| `cloudwatch-v2-metrics.tf` | +400 | New file | Low |
| `outputs.tf` | +4 (optional) | New output | Low |
| **Total** | **+418 lines** | **Config only** | **Low** |

---

## Verification Commands

**Before applying:**
```bash
cd terraform/environments/production

# Check syntax
terraform fmt -check

# Validate configuration
terraform validate

# Preview changes
terraform plan -out=v2-migration.tfplan

# Expected output:
# Plan: 11 to add, 1 to change, 0 to destroy
#
# Changes:
# + 7 CloudWatch metric filters
# + 3 CloudWatch alarms
# + 1 CloudWatch dashboard
# ~ 1 ECS task definition (environment variable added)
```

**After applying:**
```bash
# Verify variable
terraform output x402_version_support
# Expected: v1,v2

# Verify dashboard exists
aws cloudwatch get-dashboard \
  --dashboard-name facilitator-x402-v2-migration \
  --region us-east-2

# Get dashboard URL
terraform output v2_migration_dashboard_url
```

---

## Rollback Instructions

**If you need to revert these changes:**

```bash
# Revert files
git checkout HEAD -- variables.tf main.tf

# Remove new file
git rm cloudwatch-v2-metrics.tf

# Apply previous configuration
terraform plan -out=rollback.tfplan
terraform apply rollback.tfplan

# This will:
# - Remove CloudWatch metrics/dashboard
# - Remove X402_VERSION_SUPPORT env variable
# - Revert to v1-only configuration
```

**Note:** You'll also need to rollback the ECS task definition:
```bash
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --task-definition facilitator-production:<PREVIOUS_REVISION> \
  --force-new-deployment \
  --region us-east-2
```

---

## Testing Changes Locally (Optional)

**If you want to test without applying to production:**

```bash
# Option 1: Use terraform plan
terraform plan -out=v2-test.tfplan
# Review the plan output carefully

# Option 2: Use terraform workspace (if configured)
terraform workspace new v2-test
terraform apply
# Test in isolated workspace
terraform workspace select default
terraform workspace delete v2-test

# Option 3: Use terraform -target for incremental testing
terraform apply -target=aws_cloudwatch_log_metric_filter.x402_v1_requests
# Apply one resource at a time
```

---

## Cost Estimate

```bash
# Before deployment, estimate costs
terraform plan -out=v2-migration.tfplan
terraform show -json v2-migration.tfplan | jq '.resource_changes'

# Expected cost increase:
# + 7 CloudWatch metric filters @ $0.50/month = $3.50
# + 3 CloudWatch alarms @ $0.10/month = $0.30
# + 1 CloudWatch dashboard @ $0/month (free tier) = $0
# Total: ~$4/month
```

---

## Deployment Checklist

- [ ] Backup current `variables.tf`, `main.tf`, `outputs.tf`
- [ ] Add `x402_version_support` variable to `variables.tf`
- [ ] Add `X402_VERSION_SUPPORT` to `main.tf` environment block
- [ ] Verify `cloudwatch-v2-metrics.tf` file exists
- [ ] Run `terraform fmt` to format files
- [ ] Run `terraform validate` to check syntax
- [ ] Run `terraform plan` and review changes
- [ ] Save plan output: `terraform plan -out=v2-migration.tfplan`
- [ ] Apply changes: `terraform apply v2-migration.tfplan`
- [ ] Verify outputs: `terraform output v2_migration_dashboard_url`
- [ ] Open CloudWatch dashboard and verify widgets load
- [ ] Update ECS service with new task definition
- [ ] Monitor deployment for 24 hours

---

## Timeline

**Total Time:** ~15 minutes

| Step | Time | Cumulative |
|------|------|------------|
| Edit `variables.tf` | 2 min | 2 min |
| Edit `main.tf` | 1 min | 3 min |
| Run `terraform fmt` | 10 sec | 3 min |
| Run `terraform validate` | 10 sec | 3 min |
| Run `terraform plan` | 30 sec | 4 min |
| Review plan output | 3 min | 7 min |
| Run `terraform apply` | 5 min | 12 min |
| Verify outputs | 2 min | 14 min |
| Open dashboard | 1 min | 15 min |

---

## Final Notes

- These changes are **additive only** - no resources are destroyed
- Existing infrastructure (VPC, ALB, ECS) is **unchanged**
- Rollback is **safe and fast** (revert files, apply)
- Cost increase is **negligible** (~$4/month)

**Ready to deploy!**
