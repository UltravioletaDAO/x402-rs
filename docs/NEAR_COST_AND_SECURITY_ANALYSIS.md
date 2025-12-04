# NEAR Protocol Integration - Cost & Security Analysis

**Date**: 2025-12-03
**Version**: 1.0
**Scope**: AWS infrastructure costs and security considerations for NEAR Protocol support

---

## Cost Analysis

### Current Infrastructure Baseline

**Monthly costs** (before NEAR integration):
```
ECS Fargate (1 vCPU, 2GB RAM):     ~$20.00/month
Application Load Balancer:         ~$16.00/month
NAT Gateway:                       ~$32.00/month
Route53 (hosted zone + queries):    ~$1.00/month
ACM Certificate:                    $0.00/month (free)
CloudWatch Logs (7-day retention):  ~$2.00/month
Data transfer:                      ~$1.00/month
ECS Container Insights:             ~$1.00/month
---------------------------------------------------
TOTAL:                             ~$43-48/month
```

### NEAR Integration Cost Impact

#### New AWS Resources

**Secrets Manager** (2 new secrets):
- Base cost: $0.40/secret/month × 2 = $0.80/month
- API calls: ~10,000 calls/month (ECS task restarts)
  - First 10,000 API calls: $0.05 per 10,000 = $0.05/month
- **Total Secrets Manager delta**: +$0.85/month

**CloudWatch Metrics** (custom metrics):
- 5 new metric filters (free - based on log data already collected)
- 2 CloudWatch alarms (first 10 alarms free)
- 1 CloudWatch dashboard (first 3 dashboards free)
- **Total CloudWatch delta**: +$0.00/month

**NEAR RPC Endpoints**:
- Using public NEAR RPC (https://rpc.mainnet.near.org): FREE
- Using public testnet RPC (https://rpc.testnet.near.org): FREE
- Data transfer: Already included in ECS NAT Gateway costs
- **Total RPC delta**: +$0.00/month

#### Projected Monthly Cost After NEAR Integration

```
Previous infrastructure:           $43.00/month
Secrets Manager (2 new):           +$0.85/month
CloudWatch (custom metrics):       +$0.00/month
NEAR RPC (public endpoints):       +$0.00/month
---------------------------------------------------
NEW TOTAL:                         $43.85/month
```

**Delta**: +$0.85/month (+1.98% increase)

### Cost Optimization Strategies

#### Current Optimizations (Already Applied)

1. **NAT Gateway**: Using single NAT Gateway in one AZ (not multi-AZ)
   - Saves: ~$32/month per additional NAT Gateway

2. **VPC Endpoints**: Disabled (not cost-effective at current scale)
   - Would cost: ~$35/month for S3, ECR, Secrets Manager endpoints
   - Current data transfer via NAT: ~$1/month

3. **Fargate Spot**: Not used (stability > cost savings for payment facilitator)
   - Would save: ~30% (~$6/month) but risks task eviction

4. **CloudWatch Logs**: 7-day retention (not 30-day)
   - Saves: ~$5/month compared to 30-day retention

#### Future Optimizations (If NEAR Volume Increases)

1. **Reserved Capacity** (if predictable workload):
   - ECS Fargate Savings Plan: Up to 50% discount
   - Requires: 1-year commitment
   - Potential savings: ~$10/month

2. **Premium NEAR RPC** (only if rate-limited):
   - Infura NEAR mainnet: $50/month (100K requests/day)
   - Alchemy NEAR mainnet: $49/month (unlimited requests)
   - **Recommendation**: Stay on public RPC until rate-limited

3. **S3 Intelligent-Tiering** (for Terraform state):
   - Current cost: <$0.10/month (negligible)
   - Not worth the complexity

### Cost Monitoring

**CloudWatch Cost Anomaly Detection**:
```bash
# Enable cost anomaly detection for facilitator
aws ce create-anomaly-monitor \
  --anomaly-monitor Name=facilitator-cost-monitor,MonitorType=DIMENSIONAL \
  --region us-east-1

# Alert on >10% cost increase
aws ce create-anomaly-subscription \
  --anomaly-subscription Name=facilitator-cost-alerts,Threshold=10,Frequency=DAILY \
  --region us-east-1
```

**Cost Allocation Tags** (already applied):
- `Project=facilitator`
- `Environment=production`
- `ManagedBy=terraform`
- `Chain=near` (new tag for NEAR-specific resources)

**Monthly Cost Report**:
```bash
# Get facilitator costs for current month
aws ce get-cost-and-usage \
  --time-period Start=$(date -d "$(date +%Y-%m-01)" +%Y-%m-%d),End=$(date +%Y-%m-%d) \
  --granularity MONTHLY \
  --metrics BlendedCost \
  --group-by Type=TAG,Key=Project \
  --filter file://<(echo '{
    "Tags": {
      "Key": "Project",
      "Values": ["facilitator"]
    }
  }') \
  --region us-east-1
```

---

## Security Analysis

### NEAR Key Management

#### Secret Storage

**Encryption at Rest**:
- AWS Secrets Manager uses AWS KMS encryption (AES-256)
- Keys encrypted with AWS-managed KMS key: `aws/secretsmanager`
- Alternative: Use customer-managed KMS key for additional control

**Access Control**:
- IAM policy grants `secretsmanager:GetSecretValue` ONLY to:
  - ECS task execution role: `facilitator-production-ecs-execution`
- No other users/roles have access by default

**Audit Trail**:
- All secret access logged to AWS CloudTrail
- Retention: 90 days (CloudTrail default)

**Best Practices Applied**:
- ✅ Separate secrets for mainnet and testnet
- ✅ Secrets NOT exposed as environment variables (loaded dynamically)
- ✅ Secrets NOT logged (application redacts private keys)
- ✅ Secrets tagged with `Chain=near` for compliance tracking

#### Secret Rotation

**Current Rotation Policy**:
- Manual rotation (no automatic rotation for NEAR keys)
- Recommended frequency: Every 90 days (mainnet), every 30 days (testnet)

**Rotation Procedure**:
1. Generate new NEAR keypair
2. Fund new wallet with NEAR tokens
3. Update testnet secret first (test thoroughly)
4. Update mainnet secret
5. Transfer remaining funds from old wallet to new wallet
6. Monitor for 24 hours
7. Delete old keypair

**Automated Rotation** (future enhancement):
```bash
# Lambda function to rotate NEAR keys
# Triggered by AWS Secrets Manager rotation schedule
# See: scripts/rotate-near-keys.py (to be created)
```

### Network Security

#### VPC Configuration

**Inbound Traffic**:
- ALB Security Group: Allow HTTPS (443) and HTTP (80) from 0.0.0.0/0
- ECS Tasks Security Group: Allow 8080 ONLY from ALB security group

**Outbound Traffic**:
- ECS Tasks Security Group: Allow ALL outbound (for NEAR RPC calls)
- Alternative (least privilege): Allow ONLY 443 to NEAR RPC IPs
  - Challenge: NEAR RPC uses dynamic IPs (CDN)

**NEAR RPC Connectivity**:
- Public endpoints: https://rpc.mainnet.near.org (HTTPS/443)
- No VPC endpoint available (NEAR is not an AWS service)
- Traffic flow: ECS Task → NAT Gateway → Internet → NEAR RPC

#### TLS/SSL

**ALB to Client**:
- TLS 1.3 (policy: ELBSecurityPolicy-TLS13-1-2-2021-06)
- Certificate: AWS ACM (auto-renewed)

**ECS Task to NEAR RPC**:
- HTTPS (TLS 1.2+) enforced by application code
- Certificate verification: Enabled (validates NEAR RPC cert)

### IAM Security

#### Principle of Least Privilege

**ECS Task Execution Role** (bootstrapping):
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "secretsmanager:GetSecretValue"
      ],
      "Resource": [
        "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-evm-private-key-*",
        "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-solana-keypair-*",
        "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-rpc-mainnet-*",
        "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-rpc-testnet-*",
        "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-mainnet-keypair-*",
        "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-testnet-keypair-*"
      ]
    }
  ]
}
```

**ECS Task Role** (application runtime):
- Currently: No additional permissions (empty role)
- Future: Add permissions if needed for CloudWatch custom metrics, S3, etc.

#### IAM Policy Recommendations

**Deny High-Risk Actions**:
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Deny",
      "Action": [
        "secretsmanager:DeleteSecret",
        "secretsmanager:UpdateSecret",
        "secretsmanager:PutSecretValue"
      ],
      "Resource": "arn:aws:secretsmanager:us-east-2:518898403364:secret:facilitator-near-*"
    }
  ]
}
```
Apply this to all users except the deployer role.

### Application Security

#### NEAR Key Handling in Code

**Security Requirements for Rust Application**:

1. **Never Log Private Keys**:
```rust
// Good: Redacted logging
tracing::info!("Loaded NEAR private key: [REDACTED]");

// Bad: Exposes key in logs
tracing::debug!("NEAR key: {}", near_key);
```

2. **Securely Parse Private Keys**:
```rust
// Use zeroizing memory for private key parsing
use zeroize::Zeroizing;

let private_key = Zeroizing::new(
    std::env::var("NEAR_PRIVATE_KEY_MAINNET")
        .expect("NEAR_PRIVATE_KEY_MAINNET not found")
);

// Key is automatically zeroed when dropped
```

3. **Validate Key Format**:
```rust
// Ensure key is valid ed25519 format
if !private_key.starts_with("ed25519:") {
    return Err("Invalid NEAR private key format");
}
```

4. **Constant-Time Operations**:
```rust
// Use constant-time comparison for secrets
use subtle::ConstantTimeEq;

// Avoid timing attacks when comparing signatures
```

#### RPC Security

**Public RPC Risks**:
- Rate limiting: Public endpoints may throttle requests
- Availability: No SLA guarantees
- Data integrity: Man-in-the-middle attacks (mitigated by HTTPS)

**Mitigation**:
- HTTPS verification enforced (validates NEAR RPC certificate)
- Retry logic with exponential backoff (handles transient failures)
- Circuit breaker pattern (future enhancement)

**Premium RPC Benefits** (if upgraded):
- Dedicated rate limits
- 99.9% SLA
- DDoS protection
- Real-time alerts

### Compliance & Audit

#### AWS CloudTrail Logging

**Events Logged**:
- `secretsmanager:GetSecretValue` - When ECS task reads NEAR keys
- `ecs:UpdateService` - When task definition changes
- `iam:GetRolePolicy` - When IAM policies are accessed

**Log Retention**:
- CloudTrail: 90 days (default)
- Recommendation: Enable S3 export for long-term retention (1+ year)

**Audit Query**:
```bash
# List all NEAR secret access in last 7 days
aws cloudtrail lookup-events \
  --lookup-attributes AttributeKey=ResourceName,AttributeValue=facilitator-near-mainnet-keypair \
  --start-time $(date -d '7 days ago' +%s) \
  --region us-east-2
```

#### Secrets Manager Audit

**Compliance Questions**:

| Question | Answer |
|----------|--------|
| Who has access to NEAR secrets? | Only ECS task execution role `facilitator-production-ecs-execution` |
| Are secrets encrypted? | Yes, AES-256 via AWS KMS |
| Are secrets logged? | No, application redacts private keys |
| How often are secrets rotated? | Every 90 days (mainnet), every 30 days (testnet) |
| Are secrets backed up? | Yes, via AWS Secrets Manager versioning |
| Can secrets be recovered? | Yes, 30-day recovery window after deletion |

### Incident Response

#### Scenario 1: NEAR Secret Compromised

**Detection**:
- Unauthorized withdrawals from NEAR wallet
- Anomalous CloudTrail activity (secret accessed by unknown IP)

**Response Procedure**:
1. **Immediate** (within 5 minutes):
   - Revoke old NEAR secret: `aws secretsmanager delete-secret --secret-id facilitator-near-mainnet-keypair --force-delete-without-recovery --region us-east-2`
   - Generate new NEAR keypair
   - Fund new wallet
   - Create new secret with new keypair

2. **Short-term** (within 30 minutes):
   - Update ECS task definition with new secret ARN
   - Force new deployment: `aws ecs update-service --force-new-deployment`
   - Monitor logs for successful key loading

3. **Long-term** (within 24 hours):
   - Investigate compromise source (CloudTrail logs)
   - Transfer remaining funds from compromised wallet (if any)
   - Review IAM policies for overly permissive access
   - Consider switching to hardware security module (HSM)

#### Scenario 2: NEAR RPC Outage

**Detection**:
- High error rate in CloudWatch metrics
- `NEARRPCError` alarm triggered

**Response Procedure**:
1. **Verify outage**: Check NEAR status page (https://status.near.org)
2. **Switch to backup RPC**: Update `RPC_URL_NEAR_MAINNET` to alternative endpoint
3. **Premium RPC**: If prolonged outage, upgrade to Infura/Alchemy

**Backup RPC Endpoints**:
```
Primary:   https://rpc.mainnet.near.org
Backup 1:  https://near-mainnet.infura.io/v3/PUBLIC
Backup 2:  https://near-mainnet.api.alchemy.com/v2/PUBLIC
Backup 3:  https://endpoints.omniatech.io/v1/near/mainnet/public
```

#### Scenario 3: Secrets Manager Unavailable

**Detection**:
- ECS tasks fail to start
- Error: `ResourceInitializationError: unable to pull secrets`

**Response Procedure**:
1. **Check AWS Service Health**: https://status.aws.amazon.com
2. **Verify IAM permissions**: Ensure task execution role has `secretsmanager:GetSecretValue`
3. **Regional failover**: If us-east-2 Secrets Manager is down, consider multi-region deployment
4. **Temporary workaround**: Use environment variables (NOT RECOMMENDED for production)

### Security Recommendations

#### High Priority (Implement within 1 month)

1. **Enable AWS Config**:
   - Track IAM policy changes
   - Alert on non-compliant configurations
   - Cost: ~$2/month

2. **Enable GuardDuty**:
   - Detect compromised IAM credentials
   - Alert on unusual API activity
   - Cost: ~$5/month (14-day free trial)

3. **Secrets Manager Rotation Schedule**:
   - Automate NEAR key rotation every 90 days
   - Test rotation procedure on testnet first

#### Medium Priority (Implement within 3 months)

1. **VPC Flow Logs**:
   - Monitor all network traffic from ECS tasks
   - Detect anomalous connections to unknown IPs
   - Cost: ~$3/month (with CloudWatch Logs destination)

2. **AWS WAF on ALB**:
   - Protect against DDoS attacks
   - Rate limiting per IP address
   - Cost: ~$10/month

3. **Multi-Region Disaster Recovery**:
   - Replicate secrets to us-west-2
   - Standby ECS cluster in secondary region
   - Cost: ~$50/month (only active during failover)

#### Low Priority (Consider for future)

1. **AWS Nitro Enclaves**:
   - Hardware-isolated environment for NEAR key operations
   - Requires Fargate on EC2 (not supported on Fargate)
   - Alternative: AWS CloudHSM (expensive: ~$1,200/month)

2. **Zero-Knowledge Proofs**:
   - Verify NEAR transactions without exposing keys
   - Requires significant code changes

---

## Security Checklist

### Pre-Deployment

- [ ] NEAR secrets created in AWS Secrets Manager (us-east-2)
- [ ] Secrets encrypted with AWS-managed KMS key
- [ ] IAM policy grants access ONLY to ECS task execution role
- [ ] NEAR wallet funded with gas tokens (mainnet and testnet)
- [ ] RPC URLs use HTTPS (TLS verification enabled)

### Post-Deployment

- [ ] CloudTrail logging verified for NEAR secret access
- [ ] CloudWatch alarms created for NEAR RPC errors
- [ ] ECS task logs show NEAR keys loaded successfully (redacted)
- [ ] NEAR wallet balance monitoring configured
- [ ] Incident response procedure documented

### Ongoing Maintenance

- [ ] Monthly: Review CloudTrail logs for unauthorized secret access
- [ ] Monthly: Check NEAR wallet balances
- [ ] Quarterly: Rotate NEAR testnet keypair (practice)
- [ ] Quarterly: Review IAM policies for least privilege
- [ ] Annually: Rotate NEAR mainnet keypair

---

## Cost Summary

**Baseline**: $43.00/month
**After NEAR Integration**: $43.85/month
**Delta**: +$0.85/month (+1.98%)

**Cost Breakdown**:
- AWS Secrets Manager (2 secrets): +$0.85/month
- CloudWatch custom metrics: $0.00/month (free tier)
- NEAR RPC (public endpoints): $0.00/month

**Projected Annual Cost**: $526/year (from $516/year)

---

## Conclusion

**Cost Impact**: Negligible (+$0.85/month)
- Total infrastructure cost remains under $45/month
- No operational cost for NEAR RPC (using public endpoints)
- Room for growth before needing premium RPC ($50/month)

**Security Posture**: Strong
- Secrets encrypted at rest (AWS KMS)
- Least privilege IAM policies
- Network isolation (private subnets)
- Audit trail enabled (CloudTrail)
- Incident response procedures documented

**Recommendations**:
1. Monitor NEAR RPC performance for 30 days before considering premium endpoints
2. Enable AWS GuardDuty for enhanced threat detection
3. Implement automated NEAR key rotation within 3 months
4. Review security posture quarterly

---

**Document Version**: 1.0
**Last Updated**: 2025-12-03
**Next Review**: 2025-03-03
