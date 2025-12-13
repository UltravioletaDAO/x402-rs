# Zama Facilitator (Testnet) - Terraform Infrastructure

This directory contains Terraform configuration for the **x402-zama FHE Payment Facilitator** deployed on AWS Lambda.

## Overview

- **Service**: x402-zama TypeScript facilitator for FHE (Fully Homomorphic Encryption) payments
- **Network**: Ethereum Sepolia testnet (Chain ID: 11155111)
- **Architecture**: AWS Lambda + API Gateway HTTP API (v2)
- **Domain**: `zama-facilitator.ultravioletadao.xyz`
- **Cost Estimate**: ~$15/month

## Architecture Components

```
┌─────────────────────────────────────────────────────────────┐
│                     Internet (HTTPS)                        │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
                   ┌─────────────────┐
                   │   Route53 DNS   │
                   │  A Record       │
                   └────────┬────────┘
                            │
                            ▼
                   ┌─────────────────┐
                   │  ACM Certificate│
                   │  (TLS 1.2)      │
                   └────────┬────────┘
                            │
                            ▼
            ┌───────────────────────────────┐
            │  API Gateway HTTP API (v2)    │
            │  - CORS enabled               │
            │  - CloudWatch access logs     │
            └───────────────┬───────────────┘
                            │
                            ▼
            ┌───────────────────────────────┐
            │  Lambda Function              │
            │  - Node.js 20.x               │
            │  - 1GB RAM, 30s timeout       │
            │  - Provisioned Concurrency: 1 │
            └────────┬──────────────────────┘
                     │
                     ├─→ Secrets Manager (Sepolia RPC)
                     ├─→ CloudWatch Logs (14 days)
                     └─→ Zama Relayer (external)
```

## Infrastructure Resources

### Lambda Function
- **Runtime**: Node.js 20.x
- **Memory**: 1024 MB (1GB) - required for FHE operations
- **Timeout**: 30 seconds - FHE decryption can be slow
- **Provisioned Concurrency**: 1 instance (mitigates cold starts)
- **Handler**: `handler.handler` (Hono + aws-lambda adapter)

### API Gateway
- **Type**: HTTP API (v2) - cheaper than REST API
- **Routes**: `$default` (catch-all route to Lambda)
- **CORS**: Enabled for `ultravioletadao.xyz` and `localhost:3000`
- **Logging**: CloudWatch access logs enabled

### Secrets Manager
- **Secret Name**: `zama-facilitator-sepolia-rpc`
- **Purpose**: Store Ethereum Sepolia RPC URL (Infura/Alchemy)
- **Format**: JSON `{"url": "https://sepolia.infura.io/v3/YOUR_API_KEY"}`

### CloudWatch
- **Lambda Logs**: `/aws/lambda/zama-facilitator-testnet` (14 day retention)
- **API Gateway Logs**: `/aws/api-gw/zama-facilitator-testnet` (14 day retention)
- **Alarms**:
  - Lambda invocation errors (>5 errors in 5 minutes)
  - Lambda duration (>24s average - approaching 30s timeout)
  - API Gateway 5xx errors (>10 errors in 5 minutes)

### Cost Management
- **AWS Budget**: $20/month limit with 80% alert threshold
- **Estimated Monthly Cost**:
  - Lambda invocations: ~$0.02 (10k/month)
  - Lambda compute: ~$5.00 (30s avg @ 1GB)
  - Provisioned Concurrency: ~$8.00 (1 instance 24/7)
  - API Gateway: ~$0.50
  - CloudWatch Logs: ~$1.00
  - Secrets Manager: ~$0.40
  - **Total**: ~$15/month

## Deployment Instructions

### Prerequisites

1. AWS CLI configured with credentials for account `518898403364`
2. Terraform >= 1.0 installed
3. Route53 hosted zone for `ultravioletadao.xyz` (already exists)
4. Lambda deployment package (`handler.zip`) ready

### Step 1: Review and Customize Variables

Edit `terraform.tfvars` (create if doesn't exist):

```hcl
# Optional: override defaults
budget_alert_emails = ["your-email@ultravioletadao.xyz"]
enable_provisioned_concurrency = true  # Set false to save $8/month
lambda_memory_size = 1024  # Increase if FHE ops need more memory
```

### Step 2: Deploy Infrastructure

```bash
# Navigate to this directory
cd terraform/environments/zama-testnet

# Initialize Terraform (already done)
terraform init

# Review changes
terraform plan -out=zama-testnet.tfplan

# Apply infrastructure
terraform apply zama-testnet.tfplan
```

### Step 3: Upload Lambda Code

**Note**: The Lambda code doesn't exist yet (Phase 2 of the plan). For now, create a placeholder:

```bash
# Create placeholder Lambda function
echo 'exports.handler = async (event) => ({ statusCode: 200, body: "Coming soon" });' > /tmp/handler.js
cd /tmp && zip handler.zip handler.js

# Upload to S3
aws s3 cp handler.zip s3://zama-facilitator-artifacts-518898403364/handler.zip

# Update Lambda function
aws lambda update-function-code \
  --function-name zama-facilitator-testnet \
  --s3-bucket zama-facilitator-artifacts-518898403364 \
  --s3-key handler.zip \
  --region us-east-2
```

### Step 4: Configure Secrets Manager

Store the Ethereum Sepolia RPC URL:

```bash
# Using Infura (replace YOUR_API_KEY)
aws secretsmanager put-secret-value \
  --secret-id zama-facilitator-sepolia-rpc \
  --secret-string '{"url":"https://sepolia.infura.io/v3/YOUR_API_KEY"}' \
  --region us-east-2

# Or using Alchemy
aws secretsmanager put-secret-value \
  --secret-id zama-facilitator-sepolia-rpc \
  --secret-string '{"url":"https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY"}' \
  --region us-east-2
```

### Step 5: Verify Deployment

```bash
# Test health endpoint (after Phase 2 code deployment)
curl https://zama-facilitator.ultravioletadao.xyz/health

# Monitor Lambda logs
aws logs tail /aws/lambda/zama-facilitator-testnet --follow --region us-east-2

# Check Lambda function status
aws lambda get-function --function-name zama-facilitator-testnet --region us-east-2
```

## DNS Configuration

The infrastructure creates:

1. **ACM Certificate**: `zama-facilitator.ultravioletadao.xyz`
   - Validated via DNS (Route53 CNAME records created automatically)
   - TLS 1.2 minimum

2. **Route53 A Record**: Points to API Gateway regional endpoint
   - Alias record (no charge)
   - Health check evaluation enabled

DNS propagation typically takes 5-10 minutes after `terraform apply`.

## Monitoring and Debugging

### CloudWatch Logs

```bash
# Lambda function logs
aws logs tail /aws/lambda/zama-facilitator-testnet --follow

# API Gateway access logs
aws logs tail /aws/api-gw/zama-facilitator-testnet --follow

# Filter for errors
aws logs filter-log-events \
  --log-group-name /aws/lambda/zama-facilitator-testnet \
  --filter-pattern "ERROR" \
  --start-time $(date -u -d '1 hour ago' +%s)000
```

### CloudWatch Metrics

Navigate to CloudWatch Console:
- **Lambda Metrics**: Invocations, Errors, Duration, Throttles
- **API Gateway Metrics**: Count, 4XXError, 5XXError, Latency

### Alarms

Current alarms (check CloudWatch Alarms console):
1. `zama-facilitator-lambda-errors-testnet` - Lambda errors >5 in 5 min
2. `zama-facilitator-lambda-duration-testnet` - Duration >24s average
3. `zama-facilitator-api-5xx-testnet` - API Gateway 5xx errors >10 in 5 min

**Note**: Email notifications require SNS topic configuration (not included in current Terraform).

## Cost Optimization

### Reduce Costs

If budget is tight, consider:

1. **Disable Provisioned Concurrency** (saves ~$8/month):
   ```hcl
   # variables.tf or terraform.tfvars
   enable_provisioned_concurrency = false
   ```
   - Trade-off: Cold starts increase to 1-3 seconds
   - Acceptable for testnet experimentation

2. **Reduce Log Retention** (minimal savings):
   ```hcl
   log_retention_days = 7  # instead of 14
   ```

3. **Use Free RPC Tier**:
   - Infura: 100k requests/day free
   - Alchemy: 300M compute units/month free
   - Sufficient for testnet usage

### Scale Costs Up

For production or high-traffic scenarios:

1. **Increase Provisioned Concurrency**:
   - Costs: $6.00/month per additional instance
   - Use case: >1000 requests/day with strict latency requirements

2. **Increase Memory**:
   - 2048 MB: ~$10/month compute cost (double current)
   - Use case: Complex FHE operations timing out

## Troubleshooting

### Lambda fails to start

**Symptom**: 503 errors from API Gateway

**Check**:
1. Lambda deployment package uploaded: `aws lambda get-function --function-name zama-facilitator-testnet`
2. CloudWatch logs for initialization errors: `aws logs tail /aws/lambda/zama-facilitator-testnet`
3. IAM permissions: Lambda execution role can access Secrets Manager

**Fix**:
```bash
# Verify IAM policy
aws iam get-role-policy \
  --role-name zama-facilitator-lambda-testnet \
  --policy-name secrets-access
```

### Cold start latency >3 seconds

**Symptom**: Slow initial requests

**Fix**:
1. Enable Provisioned Concurrency (already enabled by default)
2. Increase `provisioned_concurrency_count` to 2 instances
3. Optimize Lambda bundle size (remove unused dependencies)

### CORS errors from frontend

**Symptom**: Browser blocks requests with CORS error

**Check**:
1. Origin in `cors_origins` variable: Default allows `https://ultravioletadao.xyz` and `http://localhost:3000`
2. Preflight OPTIONS requests succeed: `curl -X OPTIONS -H "Origin: https://ultravioletadao.xyz" https://zama-facilitator.ultravioletadao.xyz/verify -v`

**Fix**:
```hcl
# Add additional origins
cors_origins = "https://ultravioletadao.xyz,http://localhost:3000,https://app.ultravioletadao.xyz"
```

### Budget alert triggered

**Symptom**: Email alert at 80% of $20/month budget

**Check**:
1. Provisioned Concurrency not accidentally scaled: `aws lambda get-provisioned-concurrency-config --function-name zama-facilitator-testnet`
2. Unusual traffic spike: Check API Gateway metrics

**Fix**:
```bash
# Disable Provisioned Concurrency temporarily
terraform apply -var="enable_provisioned_concurrency=false"
```

## Updating Infrastructure

### Change Lambda Configuration

```bash
# Edit variables.tf or terraform.tfvars
# Example: Increase memory to 2GB
lambda_memory_size = 2048

# Apply changes
terraform plan -out=update.tfplan
terraform apply update.tfplan
```

### Deploy New Lambda Code

```bash
# Build new deployment package (Phase 2)
cd /path/to/x402-zama
pnpm install && pnpm build
# ... bundle with esbuild ...

# Upload to S3
aws s3 cp handler.zip s3://zama-facilitator-artifacts-518898403364/handler.zip

# Update function code
aws lambda update-function-code \
  --function-name zama-facilitator-testnet \
  --s3-bucket zama-facilitator-artifacts-518898403364 \
  --s3-key handler.zip \
  --region us-east-2
```

### Destroy Infrastructure

```bash
# WARNING: This will delete all resources
terraform destroy

# Confirm when prompted
```

**Note**: S3 bucket for artifacts must be empty before destruction.

## Next Steps (Phase 2)

See `docs/ZAMA_X402_INTEGRATION_PLAN.md` for:
1. Refactoring x402-zama to use Hono AWS Lambda adapter
2. Creating esbuild bundle configuration
3. Setting up CI/CD for automated deployments
4. Integration testing with real ERC7984 tokens

## Related Documentation

- **Integration Plan**: `docs/ZAMA_X402_INTEGRATION_PLAN.md`
- **Production Facilitator**: `terraform/environments/production/`
- **x402-zama Repo**: https://github.com/tomi204/x402-zama
- **Zama FHE Docs**: https://docs.zama.org/protocol

## Support

For issues or questions:
- Check CloudWatch logs for error details
- Review Zama relayer status: https://relayer.testnet.zama.cloud
- Consult Zama documentation: https://x402-fhe-docs.vercel.app/
