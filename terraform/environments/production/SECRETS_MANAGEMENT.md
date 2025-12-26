# Secrets Management - Production Infrastructure

This document explains how secrets are managed in the Facilitator production deployment and how to add new networks without breaking production.

## Problem This Solves

**Before this system:**
- Secrets were manually added to the ECS task definition
- Easy to forget secrets when adding new networks
- No single source of truth for what secrets are required
- Production deployments failed silently when secrets were missing
- IAM policies didn't include all secret ARNs, causing permission errors

**With this system:**
- All secrets defined in one place (`secrets.tf`)
- Automatic validation - Terraform will fail if secrets are missing in Secrets Manager
- IAM policy automatically includes all secrets
- Impossible to deploy without all required secrets

## Architecture

### File Organization

```
terraform/environments/production/
├── secrets.tf          # SINGLE SOURCE OF TRUTH for all secrets
├── main.tf             # Uses locals from secrets.tf
├── variables.tf        # Legacy secret name variables (kept for compatibility)
└── SECRETS_MANAGEMENT.md  # This file
```

### Secret Structure in AWS Secrets Manager

#### Wallet Secrets

All wallet secrets are JSON objects with a `private_key` field (except Stellar which is plain string):

```json
{
  "private_key": "0x...",
  "seed_phrase": "optional seed phrase for recovery"
}
```

**NEAR secrets** also include `account_id`:
```json
{
  "private_key": "ed25519:...",
  "account_id": "uvd-facilitator.near",
  "seed_phrase": "..."
}
```

**Stellar secrets** are plain strings (not JSON):
```
S<YOUR_STELLAR_SECRET_KEY_HERE>
```

#### RPC URL Secrets

RPC secrets are JSON objects with network keys:

**facilitator-rpc-mainnet:**
```json
{
  "base": "https://base-mainnet.quiknode.pro/...",
  "avalanche": "https://avalanche-mainnet.quiknode.pro/...",
  "polygon": "https://polygon-mainnet.quiknode.pro/...",
  "optimism": "https://optimism-mainnet.quiknode.pro/...",
  "celo": "https://celo-mainnet.quiknode.pro/...",
  "hyperevm": "https://...",
  "ethereum": "https://...",
  "arbitrum": "https://...",
  "unichain": "https://...",
  "solana": "https://...",
  "near": "https://..."
}
```

**facilitator-rpc-testnet:**
```json
{
  "solana-devnet": "https://...",
  "arbitrum-sepolia": "https://...",
  "near": "https://..."
}
```

## Current Network Support

### EVM Networks (18 total)

**Mainnets (9):**
- base, avalanche, polygon, optimism, celo, hyperevm, ethereum, arbitrum, unichain

**Testnets (9):**
- base-sepolia, avalanche-fuji, polygon-amoy, optimism-sepolia, celo-sepolia, hyperevm-testnet, ethereum-sepolia, arbitrum-sepolia, unichain-sepolia

**Wallet Secrets:**
- `facilitator-evm-mainnet-private-key` (JSON with `private_key` field)
- `facilitator-evm-testnet-private-key` (JSON with `private_key` field)
- `facilitator-evm-private-key` (legacy, JSON with `private_key` field)

**RPC Secrets:**
- Premium mainnet RPCs in `facilitator-rpc-mainnet` (base, avalanche, polygon, optimism, celo, hyperevm, ethereum, arbitrum, unichain)
- Free testnet RPCs in task definition environment variables
- `facilitator-rpc-testnet` only has arbitrum-sepolia currently

### Solana/SVM Networks (4 total)

**Mainnets (2):**
- solana, fogo

**Testnets (2):**
- solana-devnet, fogo-testnet

**Wallet Secrets:**
- `facilitator-solana-mainnet-keypair` (JSON with `private_key` field)
- `facilitator-solana-testnet-keypair` (JSON with `private_key` field)
- `facilitator-solana-keypair` (legacy, JSON with `private_key` field)

**RPC Secrets:**
- `facilitator-rpc-mainnet` has `solana` key
- `facilitator-rpc-testnet` has `solana-devnet` key

### NEAR Networks (2 total)

**Mainnets (1):**
- near

**Testnets (1):**
- near-testnet

**Wallet Secrets:**
- `facilitator-near-mainnet-keypair` (JSON with `private_key` and `account_id` fields)
- `facilitator-near-testnet-keypair` (JSON with `private_key` and `account_id` fields)

**RPC Secrets:**
- `facilitator-rpc-mainnet` has `near` key
- `facilitator-rpc-testnet` has `near` key

### Stellar Networks (2 total)

**Mainnets (1):**
- stellar

**Testnets (1):**
- stellar-testnet

**Wallet Secrets:**
- `facilitator-stellar-keypair-mainnet` (plain string, S... format)
- `facilitator-stellar-keypair-testnet` (plain string, S... format)

**RPC Secrets:**
- NOT YET ADDED to `facilitator-rpc-mainnet` or `facilitator-rpc-testnet`
- Using free public RPCs in task definition environment variables

## How to Add a New Network

### Step 1: Create Wallet in Secrets Manager

Choose the appropriate secret based on network type:

**For EVM chains (mainnet):**
```bash
# Already exists, just fund the wallet on your new network
# Secret: facilitator-evm-mainnet-private-key
# No action needed in Secrets Manager
```

**For EVM chains (testnet):**
```bash
# Already exists, just fund the wallet on your new network
# Secret: facilitator-evm-testnet-private-key
# No action needed in Secrets Manager
```

**For non-EVM chains (new chain family):**
```bash
# Create new mainnet wallet secret
aws secretsmanager create-secret \
  --name "facilitator-CHAINNAME-mainnet-keypair" \
  --region us-east-2 \
  --secret-string '{"private_key":"YOUR_PRIVATE_KEY_HERE"}'

# Create new testnet wallet secret
aws secretsmanager create-secret \
  --name "facilitator-CHAINNAME-testnet-keypair" \
  --region us-east-2 \
  --secret-string '{"private_key":"YOUR_PRIVATE_KEY_HERE"}'
```

### Step 2: Add RPC URL to Secrets Manager

**For premium mainnet RPC with API key:**
```bash
# Get current secret value
aws secretsmanager get-secret-value \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --query 'SecretString' --output text > /tmp/rpc-mainnet.json

# Edit the file to add your network
nano /tmp/rpc-mainnet.json
# Add: "your-network": "https://your-premium-rpc-url-with-api-key"

# Update the secret
aws secretsmanager update-secret \
  --secret-id facilitator-rpc-mainnet \
  --region us-east-2 \
  --secret-string file:///tmp/rpc-mainnet.json

# Securely delete temp file
shred -u /tmp/rpc-mainnet.json
```

**For testnet RPC (if premium):**
```bash
# Same process but with facilitator-rpc-testnet
aws secretsmanager get-secret-value \
  --secret-id facilitator-rpc-testnet \
  --region us-east-2 \
  --query 'SecretString' --output text > /tmp/rpc-testnet.json

# Edit and update
nano /tmp/rpc-testnet.json
aws secretsmanager update-secret \
  --secret-id facilitator-rpc-testnet \
  --region us-east-2 \
  --secret-string file:///tmp/rpc-testnet.json

shred -u /tmp/rpc-testnet.json
```

**For free public RPC (no API key):**
```bash
# Add directly to task definition environment variables in main.tf
# Example:
# {
#   name  = "RPC_URL_YOUR_NETWORK"
#   value = "https://free-public-rpc-url"
# }
```

### Step 3: Update secrets.tf

**For EVM networks:**
No changes needed! EVM networks use shared mainnet/testnet wallets.

**For new chain families (Algorand, Cosmos, etc):**

1. Add data source for the new secrets:

```hcl
# In secrets.tf, add to "Wallet Secrets" section:

data "aws_secretsmanager_secret" "algorand_mainnet_keypair" {
  name = "facilitator-algorand-mainnet-keypair"
}

data "aws_secretsmanager_secret" "algorand_testnet_keypair" {
  name = "facilitator-algorand-testnet-keypair"
}
```

2. Add to `wallet_secret_arns` local:

```hcl
# In secrets.tf locals block:
wallet_secret_arns = [
  # ... existing entries ...
  data.aws_secretsmanager_secret.algorand_mainnet_keypair.arn,
  data.aws_secretsmanager_secret.algorand_testnet_keypair.arn,
]
```

3. Add to `wallet_secrets` local:

```hcl
# In secrets.tf locals.wallet_secrets array:
{
  name      = "ALGORAND_PRIVATE_KEY_MAINNET"
  valueFrom = "${data.aws_secretsmanager_secret.algorand_mainnet_keypair.arn}:private_key::"
},
{
  name      = "ALGORAND_PRIVATE_KEY_TESTNET"
  valueFrom = "${data.aws_secretsmanager_secret.algorand_testnet_keypair.arn}:private_key::"
},
```

4. If using premium RPC, add to `mainnet_rpc_secrets` or `testnet_rpc_secrets`:

```hcl
# In secrets.tf locals.mainnet_rpc_secrets array:
{
  name      = "RPC_URL_ALGORAND"
  valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:algorand::"
},
```

### Step 4: Update Application Code

Add environment variable constants to `/mnt/z/ultravioleta/dao/x402-rs/src/from_env.rs`:

```rust
// For new chain family
pub const ENV_ALGORAND_PRIVATE_KEY_MAINNET: &str = "ALGORAND_PRIVATE_KEY_MAINNET";
pub const ENV_ALGORAND_PRIVATE_KEY_TESTNET: &str = "ALGORAND_PRIVATE_KEY_TESTNET";
pub const ENV_RPC_ALGORAND: &str = "RPC_URL_ALGORAND";
pub const ENV_RPC_ALGORAND_TESTNET: &str = "RPC_URL_ALGORAND_TESTNET";
```

Add network to `rpc_env_name_from_network()` match:

```rust
match network {
    // ... existing networks ...
    Network::Algorand => ENV_RPC_ALGORAND,
    Network::AlgorandTestnet => ENV_RPC_ALGORAND_TESTNET,
}
```

### Step 5: Validate and Deploy

```bash
cd terraform/environments/production

# Initialize and validate
terraform init
terraform validate

# Check what will change
terraform plan -out=facilitator.tfplan

# Review the plan carefully:
# - Verify all new secrets are included
# - Check IAM policy includes new secret ARNs
# - Verify task definition has new environment variables

# Apply changes
terraform apply facilitator.tfplan

# Force new deployment to pick up new task definition
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# Monitor deployment
aws ecs describe-services \
  --cluster facilitator-production \
  --services facilitator-production \
  --region us-east-2 \
  --query 'services[0].deployments'
```

### Step 6: Verify

```bash
# Check service is healthy
curl -s https://facilitator.ultravioletadao.xyz/health | jq

# Check new network is listed
curl -s https://facilitator.ultravioletadao.xyz/supported | jq

# Test a payment on the new network
cd tests/integration
python test_usdc_payment.py --network your-new-network
```

## Security Best Practices

### NEVER Put API Keys in Environment Variables

❌ **WRONG (exposes API key in task definition):**
```hcl
environment = [
  {
    name  = "RPC_URL_ARBITRUM"
    value = "https://node.quiknode.pro/YOUR_API_KEY_HERE/"
  }
]
```

✅ **CORRECT (uses Secrets Manager):**
```hcl
secrets = [
  {
    name      = "RPC_URL_ARBITRUM"
    valueFrom = "${data.aws_secretsmanager_secret.rpc_mainnet.arn}:arbitrum::"
  }
]
```

### Rotate Wallet Keys Regularly

See `/mnt/z/ultravioleta/dao/x402-rs/docs/WALLET_ROTATION.md` for complete procedures.

**Quick rotation:**
```bash
# 1. Generate new wallet
# 2. Fund new wallet with gas tokens
# 3. Update secret in Secrets Manager
aws secretsmanager update-secret \
  --secret-id facilitator-evm-mainnet-private-key \
  --region us-east-2 \
  --secret-string '{"private_key":"NEW_KEY_HERE"}'

# 4. Force new deployment to pick up new key
aws ecs update-service \
  --cluster facilitator-production \
  --service facilitator-production \
  --force-new-deployment \
  --region us-east-2

# 5. Wait for deployment to stabilize (2-3 minutes)
# 6. Drain old wallet (send funds to new wallet or treasury)
```

### Least Privilege IAM

The IAM policy in `main.tf` automatically grants access to all secrets defined in `secrets.tf`. This is intentional to prevent missing permissions, but follows least privilege:

- Only the ECS task execution role can access secrets
- Secrets are only accessible during container startup
- Application task role does NOT have access (separation of concerns)
- All secret access is logged in CloudTrail

## Troubleshooting

### Error: "Cannot retrieve secret"

**Symptom:**
```
Error: fetching secret value: operation error Secrets Manager: GetSecretValue
```

**Cause:** Secret name in `secrets.tf` doesn't match actual secret in Secrets Manager.

**Fix:**
```bash
# List all facilitator secrets
aws secretsmanager list-secrets \
  --region us-east-2 \
  --filters Key=name,Values=facilitator- \
  --query 'SecretList[].Name'

# Update secrets.tf to use exact name
```

### Error: "Access Denied" when starting task

**Symptom:**
```
ResourceInitializationError: unable to pull secrets or registry auth
```

**Cause:** IAM execution role doesn't have permission to access a secret.

**Fix:**
This should be automatic with the new system. If it still fails:

```bash
# Check IAM policy includes all secrets
terraform plan | grep -A 20 "secrets_access"

# Verify secrets.tf includes your secret in local.all_secret_arns
```

### Network not working after deployment

**Symptom:**
Facilitator starts but network returns "unsupported" or errors.

**Cause:** Missing environment variable in application code.

**Fix:**

1. Check ECS task definition has the secret:
```bash
aws ecs describe-task-definition \
  --task-definition facilitator-production \
  --region us-east-2 \
  --query 'taskDefinition.containerDefinitions[0].secrets'
```

2. Check application code has constant in `src/from_env.rs`

3. Check `src/network.rs` includes network in `rpc_env_name_from_network()`

### Secret value format errors

**Symptom:**
```
Failed to parse JSON from secret: invalid character 'S' looking for beginning of value
```

**Cause:** Trying to use `key::` syntax on a plain string secret (like Stellar).

**Fix:**

For JSON secrets (EVM, Solana, NEAR):
```hcl
valueFrom = "${data.aws_secretsmanager_secret.evm_mainnet_private_key.arn}:private_key::"
```

For plain string secrets (Stellar):
```hcl
valueFrom = data.aws_secretsmanager_secret.stellar_mainnet_keypair.arn
```

## Migration Guide

If you have existing secrets in the old format, use this guide to migrate:

### Old Format (before this system)

```hcl
# Old way: secrets hardcoded in main.tf
data "aws_secretsmanager_secret" "evm_private_key" {
  name = "facilitator-evm-private-key"
}

resource "aws_ecs_task_definition" "facilitator" {
  # ...
  container_definitions = jsonencode([{
    secrets = [
      {
        name      = "EVM_PRIVATE_KEY"
        valueFrom = "${data.aws_secretsmanager_secret.evm_private_key.arn}:private_key::"
      }
    ]
  }])
}
```

### New Format (after this system)

```hcl
# New way: all secrets in secrets.tf
# main.tf just references local.all_task_secrets

resource "aws_ecs_task_definition" "facilitator" {
  # ...
  container_definitions = jsonencode([{
    secrets = local.all_task_secrets
  }])
}
```

**Migration steps:**
1. Create `secrets.tf` with all current secrets
2. Update `main.tf` to use `local.all_task_secrets`
3. Run `terraform plan` to verify no changes to actual resources
4. The task definition will be recreated, but with identical secrets
5. Deploy with `terraform apply`

## Future Network Additions

### Planned Networks

**Algorand (mainnet + testnet):**
- Requires: algorand-mainnet-keypair, algorand-testnet-keypair secrets
- RPC: AlgoNode public API (free tier sufficient)
- Indexer: Required for replay protection

**Stellar (remaining work):**
- Wallets already created
- Need to add RPC URLs to `facilitator-rpc-mainnet` and `facilitator-rpc-testnet`
- Update `secrets.tf` with RPC references

**Cosmos ecosystem (Osmosis, Injective, etc):**
- Each network needs own keypair secret
- Can share RPC secret (facilitator-rpc-mainnet with cosmos-osmosis, cosmos-injective keys)

### Checklist for New Network Type

- [ ] Create wallet secrets in Secrets Manager (mainnet + testnet)
- [ ] Add RPC URLs to appropriate secret (mainnet/testnet)
- [ ] Update `secrets.tf` with data sources and locals
- [ ] Update `src/from_env.rs` with environment variable constants
- [ ] Update `src/network.rs` with RPC mapping
- [ ] Implement chain-specific logic (src/chain/)
- [ ] Run `terraform plan` and verify all secrets present
- [ ] Deploy with `terraform apply`
- [ ] Test with integration tests

## Cost Implications

**Secrets Manager Pricing (us-east-2):**
- $0.40/month per secret
- $0.05 per 10,000 API calls

**Current secrets (15 total):**
- 10 wallet secrets × $0.40 = $4.00/month
- 2 RPC secrets × $0.40 = $0.80/month
- API calls: ~$0.10/month (100k calls at 1 call per container start)
- **Total: ~$5/month**

**After adding Algorand (17 secrets):**
- 12 wallet secrets × $0.40 = $4.80/month
- 2 RPC secrets × $0.40 = $0.80/month
- API calls: ~$0.10/month
- **Total: ~$6/month**

This is well within the $45/month budget constraint.

## Additional Resources

- [Terraform AWS Secrets Manager Provider Docs](https://registry.terraform.io/providers/hashicorp/aws/latest/docs/data-sources/secretsmanager_secret)
- [ECS Secrets Best Practices](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/specifying-sensitive-data-secrets.html)
- [Wallet Rotation Procedures](/mnt/z/ultravioleta/dao/x402-rs/docs/WALLET_ROTATION.md)
- [Adding New Chains Guide](/mnt/z/ultravioleta/dao/x402-rs/guides/ADDING_NEW_CHAINS.md)
