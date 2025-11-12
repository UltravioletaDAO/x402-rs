# ECS Fargate Deployment - Completion Plan

**Status**: 5/5 agents running successfully ✅
**Created**: 2025-10-26
**Last Updated**: 2025-10-26

## Current State

### ✅ Completed
- [x] All 5 Docker images built and pushed to ECR
- [x] Terraform infrastructure deployed (VPC, ALB, ECS cluster, services)
- [x] AWS Secrets Manager configured with flat JSON structure
- [x] All 5 agents running and healthy:
  - validator (port 9001, Agent ID: 4)
  - karma-hello (port 9002, Agent ID: 1)
  - abracadabra (port 9003, Agent ID: 2)
  - skill-extractor (port 9004, Agent ID: 6)
  - voice-extractor (port 9005)
- [x] Health checks passing for all services
- [x] Agents registered on-chain (Avalanche Fuji)
- [x] Deployment automation scripts created
- [x] Force image pull automation added

### ❌ Known Issues
1. **Terraform CloudWatch Dashboard validation error** (blocking `terraform apply`)
2. **DNS not configured** (custom domains unreachable)
3. **Monitoring dashboards incomplete** (dashboard error prevents visualization)
4. **No automated backup strategy**
5. **No disaster recovery plan**
6. **Cost monitoring not set up**

---

## Phase 1: Critical Fixes (Do First)

### 1.1 Fix CloudWatch Dashboard Metrics Error
**Priority**: CRITICAL
**File**: `terraform/ecs-fargate/cloudwatch.tf`
**Error**:
```
InvalidParameterInput: The dashboard body is invalid, there are 15 validation errors:
  "dataPath": "/widgets/0/properties/metrics/0",
  "message": "Should NOT have more than 2 items"
```

**Root Cause**: CloudWatch metric arrays are limited to 2 items per metric definition, but our dashboard is using 3+ items (namespace, metric name, dimensions).

**Solution**:
- [ ] Review `cloudwatch.tf` lines 187+ (dashboard definition)
- [ ] Restructure metric definitions to use correct CloudWatch Dashboard JSON schema
- [ ] Split complex metrics into multiple widgets if necessary
- [ ] Test with `terraform plan` before applying
- [ ] Apply changes: `terraform apply -auto-approve`

**References**:
- AWS CloudWatch Dashboard Body Structure: https://docs.aws.amazon.com/AmazonCloudWatch/latest/APIReference/CloudWatch-Dashboard-Body-Structure.html
- Metric widget syntax: namespace, metric name, then dimension key/value pairs

---

## Phase 2: DNS Configuration

### 2.1 Configure Route 53 Hosted Zone
**Priority**: HIGH
**Current**: DNS entries don't exist, custom domains unreachable

**Tasks**:
- [ ] Verify Route 53 hosted zone exists for `ultravioletadao.xyz`
- [ ] Create subdomain: `karmacadabra.ultravioletadao.xyz`
- [ ] Add CNAME records pointing to ALB:
  ```
  validator.karmacadabra.ultravioletadao.xyz -> karmacadabra-prod-alb-1072717858.us-east-1.elb.amazonaws.com
  karma-hello.karmacadabra.ultravioletadao.xyz -> (same)
  abracadabra.karmacadabra.ultravioletadao.xyz -> (same)
  skill-extractor.karmacadabra.ultravioletadao.xyz -> (same)
  voice-extractor.karmacadabra.ultravioletadao.xyz -> (same)
  ```
- [ ] Test DNS propagation: `nslookup validator.karmacadabra.ultravioletadao.xyz`
- [ ] Verify health endpoints via custom domains

**Alternative (if Route 53 not available)**:
- [ ] Use ALB path-based routing exclusively
- [ ] Update documentation to use ALB URLs only
- [ ] Update agent on-chain domain registrations if needed

---

## Phase 3: Monitoring & Observability

### 3.1 CloudWatch Dashboards
**Status**: Blocked by Phase 1.1 (dashboard error)

**After Fix**:
- [ ] Verify dashboard shows all 5 services
- [ ] Add custom metrics:
  - Request count per agent
  - Error rates
  - Payment transaction success/failure
  - GLUE token balance trends
- [ ] Add cost metrics from Cost Explorer

### 3.2 CloudWatch Alarms
**Status**: Partially implemented, needs verification

**Tasks**:
- [ ] Verify existing alarms are working:
  - High CPU (>85%)
  - High memory (>85%)
  - Error count spikes
- [ ] Add missing alarms:
  - Service task count = 0 (service down)
  - Health check failures
  - ALB target health
- [ ] Configure SNS topic for alarm notifications
- [ ] Test alarm triggers

### 3.3 Log Insights Queries
**Priority**: MEDIUM

**Tasks**:
- [ ] Create saved queries for common issues:
  - Find errors in last hour
  - Payment transaction failures
  - Agent registration failures
  - Health check failures
- [ ] Document query patterns in `OPERATIONS.md`

---

## Phase 4: Cost Optimization

### 4.1 Cost Monitoring
**Current Monthly Estimate**: $81-96/month

**Tasks**:
- [ ] Set up AWS Cost Explorer tags
- [ ] Create cost allocation tags for each agent
- [ ] Set up billing alerts:
  - Warning at $75/month
  - Alert at $100/month
- [ ] Weekly cost review process

### 4.2 Right-Sizing Review
**Current**: 0.25 vCPU / 0.5GB RAM per task

**Tasks**:
- [ ] Monitor actual CPU/memory usage for 1 week
- [ ] Identify if agents can be downsized
- [ ] Consider switching validator to on-demand (critical service)
- [ ] Review auto-scaling thresholds (currently 75% CPU, 80% memory)

### 4.3 Cost Optimization Opportunities
- [ ] Review NAT Gateway usage ($32/month) - can we reduce?
- [ ] Evaluate VPC endpoints for S3/ECR (eliminate NAT gateway traffic)
- [ ] Consider Reserved Capacity for stable workloads
- [ ] Review CloudWatch Logs retention (7 days OK, or reduce to 3?)

---

## Phase 5: Security Hardening

### 5.1 Secrets Management Audit
**Current**: Using AWS Secrets Manager with flat JSON

**Tasks**:
- [ ] Verify secrets rotation is enabled
- [ ] Set up automated key rotation (30-90 days)
- [ ] Audit IAM policies for least privilege
- [ ] Remove any overly permissive `Resource: "*"` policies
- [ ] Enable CloudTrail logging for secret access

### 5.2 Network Security
**Current**: Private subnets + security groups

**Tasks**:
- [ ] Review security group rules
- [ ] Ensure no unnecessary ports are open
- [ ] Enable VPC Flow Logs for traffic analysis
- [ ] Consider AWS WAF for ALB (if budget allows)
- [ ] Review NACL rules

### 5.3 Container Security
**Tasks**:
- [ ] Scan Docker images for vulnerabilities (AWS ECR scanning)
- [ ] Enable ECR image scanning on push
- [ ] Review base image (python:3.11-slim) for CVEs
- [ ] Consider distroless images for smaller attack surface
- [ ] Implement image signing (Docker Content Trust)

---

## Phase 6: Disaster Recovery

### 6.1 Backup Strategy
**Current**: No backups configured

**Tasks**:
- [ ] Set up automated ECR image retention policy
- [ ] Backup Terraform state (already in S3, verify versioning enabled)
- [ ] Document manual recovery procedures
- [ ] Backup AWS Secrets Manager secrets to secure location
- [ ] Export on-chain data (agent registrations, reputation scores)

### 6.2 Multi-Region Considerations
**Current**: Single region (us-east-1)

**Future Enhancement**:
- [ ] Document multi-region deployment plan
- [ ] Identify critical vs non-critical services
- [ ] Plan for failover scenarios
- [ ] Consider Route 53 health checks with failover

### 6.3 Incident Response Plan
- [ ] Document runbook for common failures:
  - Service crashes
  - Health check failures
  - Payment transaction issues
  - Out of gas (AVAX)
- [ ] Create rollback procedures
- [ ] Test recovery from disaster scenarios

---

## Phase 7: Documentation & Handoff

### 7.1 Operational Documentation
**Tasks**:
- [ ] Create `OPERATIONS.md`:
  - How to deploy new images
  - How to roll back
  - How to scale services
  - How to debug issues
  - Common troubleshooting steps
- [ ] Create architecture diagram (draw.io or mermaid)
- [ ] Document all automation scripts
- [ ] Create incident response playbook

### 7.2 Developer Onboarding
**Tasks**:
- [ ] Update main README with ECS deployment instructions
- [ ] Document local development vs production differences
- [ ] Create "Getting Started" guide for new developers
- [ ] Document deployment pipeline

### 7.3 Production Readiness Checklist
- [ ] All health checks passing
- [ ] All monitoring/alerting configured
- [ ] DNS fully functional
- [ ] Security hardening complete
- [ ] Backup/DR plan documented
- [ ] Cost monitoring active
- [ ] Incident response plan created
- [ ] Team trained on operations

---

## Phase 8: Future Enhancements

### 8.1 CI/CD Pipeline
**Priority**: MEDIUM

**Tasks**:
- [ ] Set up GitHub Actions workflow
- [ ] Automate build-push-deploy on git push
- [ ] Add automated testing before deployment
- [ ] Implement blue-green deployments
- [ ] Add rollback automation

### 8.2 Advanced Monitoring
**Priority**: LOW

**Tasks**:
- [ ] Integrate with external monitoring (Datadog, New Relic)
- [ ] Set up distributed tracing (X-Ray)
- [ ] Add custom business metrics (payments/hour, revenue)
- [ ] Create Grafana dashboards

### 8.3 Performance Optimization
**Priority**: LOW

**Tasks**:
- [ ] Implement caching layers (Redis/ElastiCache)
- [ ] Optimize Docker image sizes
- [ ] Add CDN for static assets
- [ ] Review database query performance
- [ ] Implement connection pooling

---

## Quick Reference: Current Infrastructure

### ALB Details
- **DNS**: `karmacadabra-prod-alb-1072717858.us-east-1.elb.amazonaws.com`
- **Listener**: HTTP:80 (TODO: Add HTTPS)
- **Path-based routing**: `/{agent}/health` → agent target group

### ECS Services
| Service | Port | Status | Agent ID | Domain |
|---------|------|--------|----------|--------|
| validator | 9001 | ✅ RUNNING | 4 | validator.karmacadabra.ultravioletadao.xyz |
| karma-hello | 9002 | ✅ RUNNING | 1 | karma-hello.karmacadabra.ultravioletadao.xyz |
| abracadabra | 9003 | ✅ RUNNING | 2 | abracadabra.karmacadabra.ultravioletadao.xyz |
| skill-extractor | 9004 | ✅ RUNNING | 6 | skill-extractor.karmacadabra.ultravioletadao.xyz |
| voice-extractor | 9005 | ✅ RUNNING | (TBD) | voice-extractor.karmacadabra.ultravioletadao.xyz |

### Automation Scripts
- `build-and-push.ps1` - Build and push Docker images to ECR
- `deploy-and-monitor.ps1` - Deploy services and monitor progress (includes force image pull)
- `force-image-pull.ps1` - Force ECS to pull fresh images
- `diagnose-deployment.ps1` - Comprehensive deployment diagnostics

### Key Files
- `main.tf` - ECS cluster, services, task definitions
- `vpc.tf` - Network infrastructure
- `alb.tf` - Load balancer configuration
- `iam.tf` - IAM roles and policies
- `cloudwatch.tf` - Logging and monitoring (HAS ERRORS)
- `ecr.tf` - Container registry
- `variables.tf` - Configuration values
- `outputs.tf` - Deployment outputs

---

## Next Steps (Immediate)

1. **Fix CloudWatch Dashboard** (Phase 1.1) - Blocking other work
2. **Configure DNS** (Phase 2.1) - High priority for production readiness
3. **Verify all alarms** (Phase 3.2) - Ensure we're alerted to issues
4. **Set up cost monitoring** (Phase 4.1) - Prevent billing surprises

---

## Success Criteria

This deployment is considered **production-ready** when:

- ✅ All 5 agents running and healthy
- ⬜ CloudWatch dashboards working (no errors)
- ⬜ DNS fully configured and accessible
- ⬜ All alarms configured and tested
- ⬜ Cost monitoring active with alerts
- ⬜ Security hardening complete
- ⬜ Backup/DR plan documented
- ⬜ Operational documentation complete
- ⬜ Team trained on deployment/operations

**Current Score**: 1/9 (11%)
**Target**: 9/9 (100%) for production launch
